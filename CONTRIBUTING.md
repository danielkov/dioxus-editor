# Contributing

Bug reports and focused pull requests are welcome. Please discuss substantial
API or behavior changes in an issue before implementation.

Before opening a pull request, run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS='-D warnings' cargo doc -p dioxus-editor --no-deps --all-features
```

Browser changes should also run `cd e2e && npm ci && npm test`. Keep changes
small, add regression coverage for non-trivial behavior, and update the
changelog when user-facing behavior changes.

Contributions are accepted under the repository's MIT OR Apache-2.0 license.
