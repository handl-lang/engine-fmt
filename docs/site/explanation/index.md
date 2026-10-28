# Explanation

## Why this crate exists

Formatting is projection, not semantics: every kind-to-syntax mapping already exists in the language data, so the formatter only chooses a rendering strategy (infix / prefix / delimited) plus a layout.

## Where it sits

This crate belongs to the `engine-*` family. Plugins, kits and hosts reach it
through the kit's slot, so it can be swapped without touching a language
definition.

See the [reference](/engine-fmt/reference/) for the API and the
[how-to guides](/engine-fmt/how-to/) for tasks.
