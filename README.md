# engine-fmt

Data-driven AST formatter: renders an `Ast` back to surface syntax using only
language data (`grammar` rows, `tokens`, `ast` shapes) — no hardcoded kind
strings.

An `engine-*` crate: a **tool** engine in the family (not one of the five
operation slots). See also `PreserveEmitter`, which splices untouched subtrees
from the original source to keep comments and layout.

```rust
use handl_language::Language;
use hnl_engine_fmt::{FormatEngine, FormatOptions};

let lang = Language::new();               // your assembled language
let engine = FormatEngine::new(FormatOptions::default());
let out = engine.format(&lang, &ast, root)?;   // `ast`/`root` from your parser
```

## Development

```bash
cargo run --manifest-path xtask/Cargo.toml -- gate   # fmt → clippy → test → doc
```

The repo's `xtask/` crate provides the gate tasks (`fmt`, `clippy`, `test`, `doc`,
`check`, `gate`); `ci.sh` is the gate entry. Deps resolve from git, so no
sibling checkouts are needed; a plain clone test-runs as-is.

## License

MIT.

## Documentation

<https://handl-lang.github.io/docs/engine-fmt/>
