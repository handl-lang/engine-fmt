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
//! [`FmtModule`] deliberately carries **already-lexed** tokens: stage 1
//! (lex) stays with the surface — `plugin-ext-fmt` calls
//! `hnl_engine_lex_pattern::lex` today, and the eventual `handl-plugin-codegen`
//! will do the same when it adapts `CodegenRequest` into `EmitContext`.
//! Keeping lex out of this crate preserves the ADR-0095 stage boundary
//! ("lex + Pratt, reused by the surface") and keeps `materialize` a pure
//! render: tokens in → canonical text out, no fallible front-end.
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

use std::sync::Arc;

use handl_language::{Language, Tokens};
use hnl_engine_codegen::{
    ArtifactRef, Change, CodegenError, EmitContext, Emitter, IrKind, IrModule,
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
    fn materialize(&self, ir: &FmtModule, _req: &EmitContext) -> Result<Vec<Change>, CodegenError> {
        Ok(vec![Change::text(
            ir.artifact.clone(),
            ir.formatted(&self.options),
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
