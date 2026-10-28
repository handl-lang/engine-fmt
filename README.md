# engine-fmt

Data-driven AST formatter: renders an `Ast` back to surface syntax using only
language data (`grammar` rows, `tokens`, `ast` shapes) — no hardcoded kind strings.

```rust
use handl_language::Language;
use hnl_engine_fmt::{FormatEngine, FormatOptions};

let lang = Language::new();               // your assembled language
let engine = FormatEngine::new(FormatOptions::default());
let out = engine.format(&lang, &ast, root)?;   // `ast`/`root` from your parser
```

Part of the `engine-*` family (`docs/engines`): a **tool**, not one of the five
operation slots. See also `PreserveEmitter`, which splices untouched subtrees
from the original source to keep comments and layout.

## Docs

- [Reference](https://handl-lang.github.io/docs/engine-fmt/reference/) — public API
- [How-to](https://handl-lang.github.io/docs/engine-fmt/how-to/) — task recipes
- [Explanation](https://handl-lang.github.io/docs/engine-fmt/explanation/) — design

## Development

```bash
cargo test     # deps resolve from git; no sibling checkouts needed
```

`ci.sh` is the gate: `fmt --check` → `clippy -D warnings` → `test` → `doc -D warnings`.

## License

MIT.
