# Agent guide — tool-fmt

`hnl-tool-fmt` — data-driven AST formatter: renders an `Ast` back to
surface syntax using only language data (`grammar` rows, `tokens`,
`ast` shapes) — no hardcoded kind strings. `publish = false`
(git-install only). Rust 2024.

## Layout

| path | role |
| --- | --- |
| `src/lib.rs` | crate docs + public API (`PreserveEmitter` splices untouched subtrees) |
| `src/layout.rs`, `src/codegen.rs` | layout engine + surface emission |
| `src/post_process.rs`, `src/token.rs` | post-processing + token helpers |
| `src/tests.rs`, `tests/codegen.rs` | unit + integration tests |
| `project/`, `docs/` | project data + formatter doc |
| `xtask/`, `workflows/gate.yaml`, `bupha.toml`, `.cargo/` | vendored repo gate + CI wiring |

## Contract

- Deps resolve from git — `handl-language` → `handl-lang/kernel`,
  `hnl-tool-codegen`/`handl-plugin-codegen`/`hnl-tool-lex-pattern` →
  sibling repos, `branch = "main"`.
- Formatting decisions come from `Language` data only — a new language
  is a YAML fragment change, never an engine edit (engine-genericity
  rule). `PreserveEmitter` exists so untouched subtrees keep their
  original text.

## Verification

```bash
cargo xtask gate   # fmt --check → clippy -D warnings → test → doc
```


<!-- myco:agent-awareness -->

## Agent awareness

- Session start / any work discussion: run `myco status --prompt`
  before proposing next steps — know the board first.
- User describes new work → offer to capture it (`myco todo add`,
  `issue new`, `proposal new`). Don't wait to be asked.
- Picking up a task: `myco resume <ID>` is the briefing.
- Wrapping up: `myco handoff <ID>` anything resumable; keep statuses
  honest (claimed → in-progress → blocked/done).

<!-- /myco:agent-awareness -->
