# Contributing

Bug reports and focused pull requests are welcome. Please discuss substantial
API or behavior changes in an issue before implementation.

## Setup

Install [mise](https://mise.jdx.dev/), then run these commands from the
repository root:

```sh
mise install
mise run setup
```

`mise.toml` pins Rust, Node.js, and the Dioxus CLI to versions compatible with
the checked-out editor branch.

## Commands

```sh
mise run test       # Rust and browser tests
mise run test:rust  # Rust tests only
mise run test:e2e   # Playwright tests only
mise run fmt        # rustfmt check
mise run lint       # Clippy
mise run docs       # rustdoc with warnings denied
mise run serve      # browser fixture on http://127.0.0.1:18083
mise run check:rust # every Rust release check
mise run check      # every Rust and browser release check
```

Keep changes small and add regression coverage for non-trivial behavior.
Contributions are accepted under the repository's MIT OR Apache-2.0 license.
