# How-to guides

Task recipes for `tool-fmt`.

## Run the gate

```bash
./ci.sh          # fmt -> clippy -D warnings -> test -> doc -D warnings
./ci.sh test     # one stage
```

## Use it from Rust

```bash
cargo add --git https://github.com/handl-lang/tool-fmt --branch main
```

See the [reference](/tool-fmt/reference/) for the types involved and the
[explanation](/tool-fmt/explanation/) for why the crate is shaped this way.
