# engine-fmt

A data-driven AST formatter: renders an `Ast` back to surface syntax from `grammar` rows, `tokens` and `ast` shapes — no hardcoded kind strings.

## What it is

A **tool** in the `engine-*` family. Formatting is projection, not semantics: every kind-to-syntax mapping already exists in the language data, so the formatter only chooses a rendering strategy (infix / prefix / delimited) plus a layout.

## Start here

- **Reference** — [public API](/engine-fmt/reference/)
- **How-to** — [task recipes](/engine-fmt/how-to/)
- **Explanation** — [design and rationale](/engine-fmt/explanation/)

No tutorials: this repo is a building block used from Rust. End-to-end
walkthroughs live with the product that composes it — see the
[Handl platform](/handl/).
