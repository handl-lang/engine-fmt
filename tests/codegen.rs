//! Integration tests for the codegen-kernel bridge (`FmtModule` /
//! `FmtEmitter`): materialize → `Change` round-trips, comment
//! preservation, and multi-artifact change sets.
//!
//! Tests run against a minimal hand-built "mini" language (token kinds +
//! lexer rules mirroring the canonical kit's spacing categories) lexed by
//! the real data-driven lexer — the same `TokenFormatter` +
//! `canonical_post_process` pipeline `plugin-ext-fmt`'s
//! `format_source_with_language` drives, so expected strings are the
//! canonical v0.1 layout applied to mini-surface syntax.

use std::path::PathBuf;
use std::sync::Arc;

use handl_language::{Language, LexerPattern, LexerRuleDef, LiteralKind, TokenKindDef};
use handl_plugin_codegen::CodegenRequest;
use hnl_tool_codegen::{
    ArtifactRef, ArtifactStore, Change, EmitContext, Emitter, Engine, IrModule, MemoryStore,
    PlanAction, Source,
};
use hnl_tool_fmt::{
    FMT_EMITTER_NAME, FMT_IR_KIND, FMT_SOURCE_NAME, FmtEmitter, FmtModule, FmtSource,
    FormatOptions, TokenFormatter, canonical_post_process, register,
};
use hnl_tool_lex_pattern::lex;

/// A minimal language with just enough token kinds / lexer rules for the
/// formatter pipeline: newline (layout), `--` line comments, identifiers,
/// numbers, one keyword, brackets, punctuation, and operators — each
/// carrying the `category`/`label` data [`classify_token`] reads.
fn mini_lang() -> Language {
    let mut lang = Language::new();

    // --- token kinds -----------------------------------------------------
    let mut newline = TokenKindDef::new("token.newline");
    newline.layout = true;
    newline.category = Some("layout".into());
    lang.tokens.merge(newline).unwrap();

    lang.tokens
        .merge(
            TokenKindDef::new("token.comment")
                .with_trivia()
                .with_category("comment"),
        )
        .unwrap();
    lang.tokens
        .merge(TokenKindDef::new("token.identifier").with_category("identifier"))
        .unwrap();
    lang.tokens
        .merge(
            TokenKindDef::new("token.number")
                .with_literal(LiteralKind::Number)
                .with_category("number"),
        )
        .unwrap();
    lang.tokens
        .merge(
            TokenKindDef::new("token.keyword.if")
                .with_label("if")
                .with_category("keyword"),
        )
        .unwrap();

    // Brackets (category drives Open*/Close* classification via label).
    for (id, label, cat) in [
        ("token.lparen", "(", "open-bracket"),
        ("token.lbracket", "[", "open-bracket"),
        ("token.lbrace", "{", "open-bracket"),
        ("token.rparen", ")", "close-bracket"),
        ("token.rbracket", "]", "close-bracket"),
        ("token.rbrace", "}", "close-bracket"),
    ] {
        lang.tokens
            .merge(TokenKindDef::new(id).with_label(label).with_category(cat))
            .unwrap();
    }

    // Punctuation.
    for (id, label) in [
        ("token.comma", ","),
        ("token.colon", ":"),
        ("token.dot", "."),
    ] {
        lang.tokens
            .merge(
                TokenKindDef::new(id)
                    .with_label(label)
                    .with_category("punctuation"),
            )
            .unwrap();
    }

    // Binary operators (space-around via the `operator` category).
    for (id, label) in [
        ("token.equals", "="),
        ("token.bind", "::"),
        ("token.plus", "+"),
        ("token.star", "*"),
    ] {
        lang.tokens
            .merge(
                TokenKindDef::new(id)
                    .with_label(label)
                    .with_category("operator"),
            )
            .unwrap();
    }

    // --- lexer rules -----------------------------------------------------
    let rule = |id: &str, kind: &str, pattern: LexerPattern, priority: i32| {
        LexerRuleDef::new(id, kind, pattern, priority)
    };
    let lit = |text: &str| LexerPattern::Literal {
        text: text.to_string(),
    };

    // `\n` first — a layout token, like the canonical kit (priority 1).
    lang.lexer_rules
        .merge(rule("lex.newline", "token.newline", lit("\n"), 1))
        .unwrap();
    // `--` line comment (Delimited, close_at_eof — canonical priority 5).
    lang.lexer_rules
        .merge(rule(
            "lex.comment",
            "token.comment",
            LexerPattern::Delimited {
                open: "--".into(),
                close: "\n".into(),
                escape: false,
                interpolation: false,
                close_at_eof: true,
                nesting_open: None,
            },
            5,
        ))
        .unwrap();
    // Keyword before identifiers (word boundary is automatic for
    // word-like literals).
    lang.lexer_rules
        .merge(rule("lex.keyword.if", "token.keyword.if", lit("if"), 15))
        .unwrap();
    // Operators.
    for (id, kind, text) in [
        ("lex.bind", "token.bind", "::"),
        ("lex.equals", "token.equals", "="),
        ("lex.plus", "token.plus", "+"),
        ("lex.star", "token.star", "*"),
    ] {
        lang.lexer_rules
            .merge(rule(id, kind, lit(text), 20))
            .unwrap();
    }
    // Punctuation (`::` is already claimed by the operator rule above).
    for (id, kind, text) in [
        ("lex.lparen", "token.lparen", "("),
        ("lex.rparen", "token.rparen", ")"),
        ("lex.lbracket", "token.lbracket", "["),
        ("lex.rbracket", "token.rbracket", "]"),
        ("lex.lbrace", "token.lbrace", "{"),
        ("lex.rbrace", "token.rbrace", "}"),
        ("lex.comma", "token.comma", ","),
        ("lex.colon", "token.colon", ":"),
        ("lex.dot", "token.dot", "."),
    ] {
        lang.lexer_rules
            .merge(rule(id, kind, lit(text), 30))
            .unwrap();
    }
    // Numbers, then identifiers (longest/priority ordering as canonical).
    lang.lexer_rules
        .merge(rule("lex.number", "token.number", LexerPattern::Number, 40))
        .unwrap();
    lang.lexer_rules
        .merge(rule(
            "lex.identifier",
            "token.identifier",
            LexerPattern::CharClass {
                start_chars: "a-zA-Z_".into(),
                continue_chars: "a-zA-Z0-9_".into(),
            },
            50,
        ))
        .unwrap();

    lang
}

fn lang() -> &'static Arc<Language> {
    static LANG: std::sync::OnceLock<Arc<Language>> = std::sync::OnceLock::new();
    LANG.get_or_init(|| Arc::new(mini_lang()))
}

/// Lex `src` and wrap it in a [`FmtModule`] targeting `artifact`.
fn module(artifact: &str, src: &str) -> FmtModule {
    let tokens = lex(lang(), src).expect("source lexes");
    FmtModule::new(artifact, src, tokens, lang().clone())
}

/// The surface pipeline, spelled out independently of `FmtModule`:
/// `TokenFormatter::canonical` + `canonical_post_process` — what
/// `format_source_with_language` runs in `plugin-ext-fmt`.
fn surface_format(src: &str) -> String {
    let tokens = lex(lang(), src).expect("source lexes");
    if tokens.as_slice().is_empty() {
        return String::new();
    }
    let rendered = TokenFormatter::canonical(FormatOptions::default().indent_size).format(
        src,
        tokens.as_slice(),
        lang(),
    );
    canonical_post_process(&rendered)
}

fn materialized_text(ir: &FmtModule) -> String {
    let changes = FmtEmitter::canonical()
        .materialize(ir, &EmitContext::new())
        .expect("materialize");
    assert_eq!(changes.len(), 1, "fmt emits exactly one Change per module");
    let Change::Put {
        artifact, content, ..
    } = &changes[0]
    else {
        panic!("fmt must emit a Put, got {:?}", changes[0]);
    };
    assert_eq!(artifact, &ir.artifact);
    String::from_utf8(content.clone()).expect("fmt output is UTF-8")
}

#[test]
fn ir_kind_is_fmt_module() {
    let ir = module("main.hnl", "x::1\n");
    assert_eq!(ir.kind().as_str(), FMT_IR_KIND);
    assert_eq!(FMT_IR_KIND, "fmt.module");
}

#[test]
fn materialize_matches_surface_pipeline() {
    let cases = [
        "x::1\n",
        "x  ::  1\n",
        "x::1\ny::2\n",
        "f( 1,2 )\n",
        "x::{ a:1 }\n",
        "if x { y }\n",
        "x::[1,2,3]\n",
        "x::f(1)+g(2)*3\n",
        "x::1 -- tail\n",
        "-- lead\nx::1\n",
        "-- only a comment\n",
        "x::1\n\n\n\ny::2\n",
        "",
        "   \n\n  \n",
    ];
    for src in cases {
        let ir = module("main.hnl", src);
        assert_eq!(
            materialized_text(&ir),
            surface_format(src),
            "materialize diverged from the surface pipeline for {src:?}"
        );
    }
}

#[test]
fn materialize_produces_canonical_output() {
    let ir = module("main.hnl", "x  ::  1\n");
    assert_eq!(materialized_text(&ir), "x :: 1\n");
}

#[test]
fn comments_survive_formatting() {
    // Hard requirement: `--` comments are tokens in the stream; the
    // token-stream renderer preserves them where an Ast re-render would
    // drop them.
    let src = "-- a note about x.\nx::1\n";
    let ir = module("main.hnl", src);
    let out = materialized_text(&ir);
    assert_eq!(out, "-- a note about x.\nx :: 1\n");
    assert!(out.contains("-- a note about x."));
}

#[test]
fn trailing_comment_survives() {
    let ir = module("main.hnl", "x::1 -- tail\n");
    assert_eq!(materialized_text(&ir), "x :: 1 -- tail\n");
}

#[test]
fn comment_only_source() {
    let ir = module("notes.hnl", "-- just a comment\n");
    assert_eq!(materialized_text(&ir), "-- just a comment\n");
}

#[test]
fn empty_source_materializes_empty_put() {
    for src in ["", "   \n\n  \n"] {
        let ir = module("empty.hnl", src);
        assert_eq!(materialized_text(&ir), "");
    }
}

#[test]
fn blank_line_between_bindings() {
    // `canonical_post_process`'s decl-group rule fires on `::` bindings.
    let ir = module("main.hnl", "x::1\ny::2\n");
    assert_eq!(materialized_text(&ir), "x :: 1\n\ny :: 2\n");
}

#[test]
fn multi_file_change_set() {
    // Two modules → two independent `Change::text`s, one per artifact.
    let a = module("src/a.hnl", "x::1\n");
    let b = module("src/b.hnl", "-- note\ny::2\n");
    let emitter = FmtEmitter::canonical();
    let ctx = EmitContext::new();

    let changes_a = emitter.materialize(&a, &ctx).expect("a materializes");
    let changes_b = emitter.materialize(&b, &ctx).expect("b materializes");

    assert_eq!(changes_a.len(), 1);
    assert_eq!(changes_b.len(), 1);
    let Change::Put {
        artifact: aa,
        content: ca,
        ..
    } = &changes_a[0]
    else {
        panic!("expected Put");
    };
    let Change::Put {
        artifact: ab,
        content: cb,
        ..
    } = &changes_b[0]
    else {
        panic!("expected Put");
    };
    assert_eq!(aa, &ArtifactRef::new("src/a.hnl"));
    assert_eq!(ab, &ArtifactRef::new("src/b.hnl"));
    assert_eq!(String::from_utf8(ca.clone()).unwrap(), "x :: 1\n");
    assert_eq!(String::from_utf8(cb.clone()).unwrap(), "-- note\ny :: 2\n");
}

#[test]
fn verify_accepts_own_change_set() {
    let ir = module("main.hnl", "x::1\n");
    let emitter = FmtEmitter::canonical();
    let changes = emitter
        .materialize(&ir, &EmitContext::new())
        .expect("materialize");
    emitter.verify(&ir, &changes).expect("verify accepts");
}

#[test]
fn verify_rejects_wrong_artifact_or_extra_changes() {
    let ir = module("main.hnl", "x::1\n");
    let emitter = FmtEmitter::canonical();

    // A Put on a different artifact.
    let foreign = vec![Change::text("other.hnl", "x :: 1\n")];
    assert!(emitter.verify(&ir, &foreign).is_err());

    // Two changes where one was promised.
    let mut doubled = emitter
        .materialize(&ir, &EmitContext::new())
        .expect("materialize");
    doubled.push(Change::text("extra.hnl", ""));
    assert!(emitter.verify(&ir, &doubled).is_err());

    // A Delete where a Put was promised.
    let deleted = vec![Change::delete("main.hnl")];
    assert!(emitter.verify(&ir, &deleted).is_err());
}

#[test]
fn materialize_is_idempotent_at_change_level() {
    // fmt(fmt(src)) == fmt(src): formatting the formatted output must be
    // a fixed point — the property `hnl fmt --check` relies on.
    let src = "-- c\nx  ::  1\ny::f( 1,2 )\n";
    let once = module("main.hnl", src);
    let once_text = materialized_text(&once);
    let twice = module("main.hnl", &once_text);
    let twice_text = materialized_text(&twice);
    assert_eq!(once_text, twice_text);
    // And it equals the independently-spelled surface pipeline.
    assert_eq!(once_text, surface_format(&once_text));
}

// ---------------------------------------------------------------------------
// FmtSource + register (PL-004 T-090): spec → IR → Change → plan
// ---------------------------------------------------------------------------

fn write_spec(dir: &std::path::Path, rel: &str, src: &str) -> PathBuf {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().expect("spec has a parent")).unwrap();
    std::fs::write(&path, src).expect("write spec");
    path
}

fn ctx_for(spec: &std::path::Path, output: &std::path::Path) -> EmitContext {
    EmitContext::new()
        .with_spec_path(spec)
        .with_output_dir(output)
}

#[test]
fn fmt_source_parse_reads_lexes_and_names_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let spec = write_spec(dir.path(), "main.hnl", "x  ::  1\n");

    let source = FmtSource::new(lang().clone());
    let ir = source
        .parse(&ctx_for(&spec, dir.path()))
        .expect("parse lifts file → FmtModule");

    assert_eq!(ir.kind().as_str(), FMT_IR_KIND);
    assert_eq!(ir.artifact, ArtifactRef::new("main.hnl"));
    assert_eq!(ir.source, "x  ::  1\n");
    // The lifted module materializes through the same emitter.
    assert_eq!(materialized_text(&ir), "x :: 1\n");
}

#[test]
fn fmt_source_artifact_keeps_path_under_store_root() {
    let dir = tempfile::tempdir().unwrap();
    let spec = write_spec(dir.path(), "src/nested/a.hnl", "y::2\n");

    let source = FmtSource::new(lang().clone());
    let ir = source.parse(&ctx_for(&spec, dir.path())).expect("parse");
    assert_eq!(ir.artifact, ArtifactRef::new("src/nested/a.hnl"));
}

#[test]
fn fmt_source_output_root_itself_is_stripped() {
    // `--output src` with spec `src/a.hnl` → artifact `a.hnl`.
    let dir = tempfile::tempdir().unwrap();
    let src_dir = dir.path().join("src");
    let spec = write_spec(&src_dir, "a.hnl", "y::2\n");

    let source = FmtSource::new(lang().clone());
    let ir = source.parse(&ctx_for(&spec, &src_dir)).expect("parse");
    assert_eq!(ir.artifact, ArtifactRef::new("a.hnl"));
}

#[test]
fn fmt_source_requires_spec_path() {
    let source = FmtSource::new(lang().clone());
    let err = source.parse(&EmitContext::new()).unwrap_err();
    assert!(matches!(err, hnl_tool_codegen::CodegenError::Parse(_)));
}

#[test]
fn fmt_source_missing_file_is_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let spec = dir.path().join("nope.hnl");
    let source = FmtSource::new(lang().clone());
    let err = source.parse(&ctx_for(&spec, dir.path())).unwrap_err();
    assert!(matches!(err, hnl_tool_codegen::CodegenError::Parse(_)));
}

#[test]
fn fmt_source_unlexable_source_is_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    // `@` matches no lexer rule in the mini language.
    let spec = write_spec(dir.path(), "bad.hnl", "x @ 1\n");
    let source = FmtSource::new(lang().clone());
    let err = source.parse(&ctx_for(&spec, dir.path())).unwrap_err();
    assert!(matches!(err, hnl_tool_codegen::CodegenError::Parse(_)));
}

#[test]
fn fmt_source_absolute_spec_outside_store_root_is_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let spec = write_spec(other.path(), "a.hnl", "x::1\n");

    let source = FmtSource::new(lang().clone());
    let err = source.parse(&ctx_for(&spec, dir.path())).unwrap_err();
    match err {
        hnl_tool_codegen::CodegenError::Parse(msg) => {
            assert!(msg.contains("not under the store root"), "{msg}");
        }
        other_err => panic!("expected Parse error, got {other_err}"),
    }
}

#[test]
fn register_seats_fmt_source_emitter_and_target() {
    let mut reg = handl_plugin_codegen::CodegenRegistry::new();
    register(&mut reg);
    assert!(reg.contains("fmt"));
    assert!(reg.target_names().contains(&"fmt"));
    assert_eq!(FMT_SOURCE_NAME, "fmt");
    assert_eq!(FMT_EMITTER_NAME, "fmt");
}

#[test]
fn registry_materialize_runs_the_full_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let spec = write_spec(dir.path(), "main.hnl", "x  ::  1\n");

    let mut reg = handl_plugin_codegen::CodegenRegistry::new();
    register(&mut reg);
    let language: &Language = lang();
    let req = CodegenRequest {
        spec: &spec,
        source_url: None,
        output: dir.path(),
        target: None,
        include: None,
        exclude: None,
        config: None,
        emit_spec: None,
        lang: language,
        cache_dir: None,
    };

    let changes = reg.materialize("fmt", &req).expect("materialize");
    assert_eq!(changes.len(), 1);
    let Change::Put {
        artifact, content, ..
    } = &changes[0]
    else {
        panic!("fmt target must emit a Put");
    };
    assert_eq!(artifact, &ArtifactRef::new("main.hnl"));
    assert_eq!(content, b"x :: 1\n");
}

#[test]
fn plan_is_the_check_semantics() {
    // `fmt --check` = desired-vs-store diff: a store already holding the
    // canonical bytes plans clean; the raw bytes plan one Update.
    let dir = tempfile::tempdir().unwrap();
    let spec = write_spec(dir.path(), "main.hnl", "x  ::  1\n");

    let mut reg = handl_plugin_codegen::CodegenRegistry::new();
    register(&mut reg);
    let language: &Language = lang();
    let req = CodegenRequest {
        spec: &spec,
        source_url: None,
        output: dir.path(),
        target: None,
        include: None,
        exclude: None,
        config: None,
        emit_spec: None,
        lang: language,
        cache_dir: None,
    };
    let changes = reg.materialize("fmt", &req).expect("materialize");

    let store = MemoryStore::new();
    let engine = Engine::new(&store);
    let artifact = ArtifactRef::new("main.hnl");

    store.write(&artifact, b"x :: 1\n").expect("seed canonical");
    let plan = engine.plan(&changes, None).expect("plan");
    assert!(plan.is_clean(), "canonical store content must plan clean");
    assert_eq!(plan.count(PlanAction::Unchanged), 1);

    store.write(&artifact, b"x  ::  1\n").expect("seed raw");
    let plan = engine.plan(&changes, None).expect("plan");
    assert!(!plan.is_clean(), "raw store content must plan a mutation");
    assert_eq!(plan.count(PlanAction::Update), 1);
    assert_eq!(plan.mutations().count(), 1);
}
