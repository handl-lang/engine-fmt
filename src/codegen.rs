//! Codegen-kernel bridge — `FmtModule` IR + [`FmtEmitter`] (T-090/T-091 prep).
//!
//! # Design decision: token-stream IR, not `Ast`
//!
//! The plan sketches `Emitter<Ast>`, but `hnl fmt` is **token-stream** based:
//! [`TokenFormatter`] + [`canonical_post_process`] consume the lexed
//! [`Tokens`] stream directly and never build an [`handl_language::Ast`].
//! That is precisely why `--` comments survive formatting — comments are
//! tokens, and the renderer preserves them. Routing fmt through an `Ast`
//! emitter ([`crate::FormatEngine`]/[`crate::PreserveEmitter`]) would
//! **drop comments** on a full re-render, violating the
//! comment-preservation requirement. The
//! honest IR is therefore the thing fmt actually is: one source artifact,
//! its lexed token stream, and the [`Language`] data it was lexed/laid out
//! against.
//!
//! # Lift boundary
//!
//! [`FmtModule`] deliberately carries **already-lexed** tokens:
//! [`FmtSource`] (the kernel [`Source`] impl) owns the stage-1 lex —
//! read file → `hnl_tool_lex_pattern::lex` → token stream — while
//! [`FmtEmitter`]/[`FmtModule::formatted`] stay a pure render: tokens in
//! → canonical text out, no fallible front-end. This keeps
//! `materialize` side-effect-free and preserves the ADR-0095 stage
//! boundary ("lex + Pratt, reused by the surface"): the surface
//! (`plugin-ext-fmt`, and now the `fmt` codegen target) and this
//! `Source` share the *same* lexer engine rather than duplicating it.
//!
//! # `Change` shape
//!
//! [`FmtEmitter::materialize`] emits exactly one [`Change::text`] per
//! module, targeting `ir.artifact`. Whether the formatted text differs
//! from the input is *not* this emitter's concern — desired-vs-current
//! diffing (and therefore `fmt --check` semantics) is the kernel
//! `Plan`/`ArtifactStore`'s job: an unchanged artifact produces a `Put`
//! whose content hashes equal, which `plan` reports as no-op.
//!
//! [`Emitter::verify`] is intentionally minimal: it checks the change set is the
//! single-artifact `Put` this emitter promised. Idempotence
//! (`fmt(fmt(src)) == fmt(src)`) is a property of the pipeline already
//! tested exhaustively in `plugin-ext-fmt`; re-checking it here would
//! require re-lexing the *output*, which this IR does not carry.
//!
//! # `fmt --check`
//!
//! There is no `--check` flag at this layer. Check semantics are the
//! kernel lifecycle's: [`FmtSource::parse`] + [`FmtEmitter::materialize`]
//! produce a desired [`Change::Put`], and `Engine::plan`/`ArtifactStore`
//! diffing reports a no-op when the formatted bytes already match the
//! store — "check" is "plan produced no changes". Drift between plan
//! and apply is caught by the kernel's `before_hash` tripwire
//! ([`CodegenError::Drift`]). The command layer (`hnl fmt --check`,
//! `hnl codegen plan`) owns the exit code.
//!
//! # Registry surface
//!
//! [`register`] mirrors the `tool-codegen-*` backend convention
//! (ADR-0128 §0.1, PL-004): it seats source id `"fmt"`, emitter id
//! `"fmt"`, and target record `"fmt"` on a
//! [`handl_plugin_codegen::CodegenRegistry`], so `hnl fmt` can run the
//! same spec → IR → `Change` → `Engine::apply` pipeline as
//! `hnl codegen` instead of the capability-JSON file→file seam.

use std::path::Path;
use std::sync::Arc;

use handl_language::{Language, Tokens};
use handl_plugin_codegen::{
    AnyEmitter, AnySource, CodegenEmitterDef, CodegenRegistry, CodegenRequest, CodegenSourceDef,
    CodegenTarget, erase_emitter, erase_source,
};
use hnl_tool_codegen::{
    ArtifactRef, Change, CodegenError, EmitContext, Emitter, IrKind, IrModule, Source,
};

use crate::{FormatOptions, TokenFormatter, canonical_post_process};

/// IR family tag for [`FmtModule`].
pub const FMT_IR_KIND: &str = "fmt.module";

/// The formatter's intermediate representation: one source artifact as a
/// lexed token stream plus the [`Language`] it formats under.
///
/// This is deliberately **not** an [`handl_language::Ast`]: the token
/// stream is what preserves comments and exact source text (see module
/// docs). Construct via [`FmtModule::new`] after lexing; the eventual
/// `handl-plugin-codegen` `Source` impl owns the lex step.
#[derive(Debug, Clone)]
pub struct FmtModule {
    /// Store key the formatted text is desired at.
    pub artifact: ArtifactRef,
    /// The original source text — `TokenFormatter` recovers per-line
    /// leading whitespace and blank lines from byte-offset gaps in it.
    pub source: String,
    /// The lexed token stream (comments included; layout tokens carry
    /// the original whitespace the renderer preserves).
    pub tokens: Tokens,
    /// Language data (tokens, grammar, lexer rules) the stream was lexed
    /// and laid out against.
    pub lang: Arc<Language>,
}

impl FmtModule {
    /// Build a module from an artifact ref, source text, its lexed token
    /// stream, and the language.
    pub fn new(
        artifact: impl Into<ArtifactRef>,
        source: impl Into<String>,
        tokens: Tokens,
        lang: Arc<Language>,
    ) -> Self {
        Self {
            artifact: artifact.into(),
            source: source.into(),
            tokens,
            lang,
        }
    }

    /// Format this module with the given options: token-stream render
    /// ([`TokenFormatter`]) followed by post-process
    /// ([`canonical_post_process`]) — the same pipeline
    /// `format_source_with_language` runs in `plugin-ext-fmt`.
    pub fn formatted(&self, options: &FormatOptions) -> String {
        // Mirror `format_source_with_language`: an empty token stream
        // (empty/whitespace-only source) formats to the empty string.
        if self.tokens.as_slice().is_empty() {
            return String::new();
        }
        let formatter = TokenFormatter::canonical(options.indent_size);
        let rendered = formatter.format(&self.source, self.tokens.as_slice(), &self.lang);
        canonical_post_process(&rendered)
    }

    /// Format with default [`FormatOptions`].
    pub fn formatted_default(&self) -> String {
        self.formatted(&FormatOptions::default())
    }
}

impl IrModule for FmtModule {
    fn kind(&self) -> IrKind {
        IrKind::new(FMT_IR_KIND)
    }
}

/// `Emitter<FmtModule>`: materialize the canonical formatted text as a
/// desired-state [`Change`] set.
///
/// Stateless — the only configuration is the [`FormatOptions`] (indent
/// width; `width` is reserved for future chain-breaking rules, matching
/// the surface's use of `FormatOptions::default()`).
#[derive(Debug, Clone, Default)]
pub struct FmtEmitter {
    /// Formatting options forwarded to [`FmtModule::formatted`].
    pub options: FormatOptions,
}

impl FmtEmitter {
    /// Create an emitter with the given options.
    pub fn new(options: FormatOptions) -> Self {
        Self { options }
    }

    /// Create an emitter with default options (2-space indent, width 100).
    pub fn canonical() -> Self {
        Self::default()
    }
}

impl Emitter<FmtModule> for FmtEmitter {
    fn materialize(&self, ir: &FmtModule, req: &EmitContext) -> Result<Vec<Change>, CodegenError> {
        Ok(vec![Change::text(
            ir.artifact.clone(),
            ir.formatted(&self.options_with(req)),
        )])
    }

    /// Structural verify gate: the change set must be exactly the single
    /// `Put` this emitter promised, targeting `ir.artifact`.
    ///
    /// Content-level verification (idempotence) needs a re-lex of the
    /// formatted output, which this token-carrying IR does not hold —
    /// see module docs. `fmt`'s idempotence is already covered by the
    /// `plugin-ext-fmt` test suite.
    fn verify(&self, ir: &FmtModule, changes: &[Change]) -> Result<(), CodegenError> {
        match changes {
            [change]
                if change.artifact() == &ir.artifact && matches!(change, Change::Put { .. }) =>
            {
                Ok(())
            }
            _ => Err(CodegenError::Verify(format!(
                "fmt emitter produced {} change(s), expected one Put on {}",
                changes.len(),
                ir.artifact
            ))),
        }
    }
}

impl FmtEmitter {
    /// Effective options for one `materialize`: the emitter's configured
    /// [`FormatOptions`] overlaid with `EmitContext::config` keys —
    /// `"indent"` (spaces per level) and `"width"` (line width). This is
    /// the documented channel request flags (`--adapter-opt`-style
    /// `KEY=VALUE`) reach emitters through; unparseable values are
    /// ignored rather than failing the emit.
    fn options_with(&self, req: &EmitContext) -> FormatOptions {
        let mut options = self.options.clone();
        if let Some(n) = req
            .config
            .get("indent")
            .and_then(|v| v.parse::<usize>().ok())
        {
            options.indent_size = n;
        }
        if let Some(n) = req
            .config
            .get("width")
            .and_then(|v| v.parse::<usize>().ok())
        {
            options.width = n;
        }
        options
    }
}

// ---------------------------------------------------------------------------
// Source + registry surface (PL-004 T-090)
// ---------------------------------------------------------------------------

/// Source id / emitter id / target name `register` seats on the
/// [`CodegenRegistry`] — the names `hnl fmt`'s desired-state wiring
/// resolves through.
pub const FMT_SOURCE_NAME: &str = "fmt";
/// Emitter id registered alongside [`FMT_SOURCE_NAME`].
pub const FMT_EMITTER_NAME: &str = "fmt";

/// `Source<FmtModule>` — the stage-1 lift: read the spec file named by
/// [`EmitContext::spec_path`], lex it into the token stream
/// [`FmtModule`] carries, and name the [`ArtifactRef`] the formatted
/// text is desired at.
///
/// Lexing *can* fail ([`CodegenError::Parse`]) — the data-driven lexer
/// rejects source no `LexerRuleDef` covers. Everything after lex is
/// infallible: [`FmtEmitter::materialize`] is a pure render.
///
/// The [`Language`] is captured at construction (the `make` factory
/// clones `CodegenRequest::lang`, the ADR-0128 §0.1 seam) — it never
/// crosses the kernel boundary inside `EmitContext`.
#[derive(Debug, Clone)]
pub struct FmtSource {
    lang: Arc<Language>,
}

impl FmtSource {
    /// Create a source that lexes/formats against `lang`.
    pub fn new(lang: Arc<Language>) -> Self {
        Self { lang }
    }
}

impl Source<FmtModule> for FmtSource {
    fn parse(&self, req: &EmitContext) -> Result<FmtModule, CodegenError> {
        let spec = req.spec_path.as_deref().ok_or_else(|| {
            CodegenError::Parse("fmt source requires EmitContext::spec_path".to_string())
        })?;
        let source = std::fs::read_to_string(spec)
            .map_err(|e| CodegenError::Parse(format!("failed to read {}: {e}", spec.display())))?;
        let tokens = hnl_tool_lex_pattern::lex(&self.lang, &source)
            .map_err(|e| CodegenError::Parse(format!("lex error in {}: {e}", spec.display())))?;
        let artifact = artifact_ref(spec, req.output_dir.as_deref())?;
        Ok(FmtModule::new(artifact, source, tokens, self.lang.clone()))
    }
}

/// Resolve the [`ArtifactRef`] `spec` formats to.
///
/// Preference order:
/// 1. `spec` relative to the store root (`req.output_dir`) — canonicalized
///    on both sides so `..`/symlink spellings still strip; covers
///    `hnl fmt src/a.hnl --output src` → `a.hnl` and `--output .` →
///    `src/a.hnl`.
/// 2. `spec` verbatim when it is already a relative path (the
///    FsStore-relative spelling).
///
/// An absolute `spec` outside the store root is an error — fmt must
/// never silently target a different file than the one it read.
fn artifact_ref(spec: &Path, output_dir: Option<&Path>) -> Result<ArtifactRef, CodegenError> {
    let canon_spec = std::fs::canonicalize(spec).unwrap_or_else(|_| spec.to_path_buf());
    if let Some(dir) = output_dir {
        let canon_dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        // Canonical pair first (handles relative `spec` under an absolute
        // cwd, absolute `spec` under the root), then the literal pair
        // (covers a store root that does not exist yet to canonicalize).
        for (base, candidate) in [(canon_dir.as_path(), canon_spec.as_path()), (dir, spec)] {
            if let Ok(rel) = candidate.strip_prefix(base)
                && !rel.as_os_str().is_empty()
            {
                return checked_ref(&rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    if spec.is_absolute() {
        return Err(CodegenError::Parse(format!(
            "fmt spec {} is not under the store root {} — pass --output <ancestor dir>",
            spec.display(),
            output_dir
                .map(|d| d.display().to_string())
                .unwrap_or_else(|| "<unset>".to_string())
        )));
    }
    checked_ref(&spec.to_string_lossy().replace('\\', "/"))
}

/// Build an [`ArtifactRef`] from a `/`-joined relative path, rejecting
/// reserved-prefix and non-relative spellings up front.
fn checked_ref(path: &str) -> Result<ArtifactRef, CodegenError> {
    let artifact = ArtifactRef::new(path);
    artifact
        .validate_fs_relative()
        .map_err(|m| CodegenError::Parse(format!("fmt artifact ref: {m}")))?;
    if artifact.is_reserved() {
        return Err(CodegenError::Parse(format!(
            "fmt artifact ref {path:?} is under the reserved {} prefix",
            hnl_tool_codegen::RESERVED_PREFIX
        )));
    }
    Ok(artifact)
}

/// Register the `fmt` source/emitter/target triple on `reg` — the same
/// link-time convention the `tool-codegen-*` backends use
/// (ADR-0128 §0.1, PL-004 T-090):
///
/// - source `"fmt"` — [`FmtSource`]: `spec_path` file → lexed
///   [`FmtModule`].
/// - emitter `"fmt"` — [`FmtEmitter::canonical`]: module → one
///   `Change::text` Put of the canonical formatted bytes.
/// - target `"fmt"` — the record composing the two, for
///   `CodegenRegistry::run`/`materialize`/`plan`.
///
/// `--check` is *not* registered here: it is `registry.plan("fmt", req)`
/// (or `materialize` + `Engine::plan`) reporting an empty diff — see the
/// module docs' "`fmt --check`" section. The `Language` reaches the
/// source/emitter through `CodegenRequest::lang` at `make` time, never
/// through `EmitContext`.
pub fn register(reg: &mut CodegenRegistry) {
    reg.register_source(CodegenSourceDef {
        name: FMT_SOURCE_NAME,
        make: make_fmt_source,
    });
    reg.register_emitter(CodegenEmitterDef {
        name: FMT_EMITTER_NAME,
        make: make_fmt_emitter,
    });
    reg.register_target(CodegenTarget {
        name: FMT_SOURCE_NAME,
        source: FMT_SOURCE_NAME,
        emitter: FMT_EMITTER_NAME,
        source_filters: false,
        // The docs pass is `SdkgenModule`-only; fmt IR carries none.
        docs: None,
        spec_input: None,
    });
}

fn make_fmt_source(req: &CodegenRequest<'_>) -> Result<Box<dyn AnySource>, String> {
    Ok(erase_source(FmtSource::new(Arc::new(req.lang.clone()))))
}

fn make_fmt_emitter(_req: &CodegenRequest<'_>) -> Result<Box<dyn AnyEmitter>, String> {
    Ok(erase_emitter(FmtEmitter::canonical()))
}
