# Browser tests

The Playwright suite drives the real Dioxus fixture in Chromium.

From the repository root:

```sh
mise install
mise run setup
mise run test:e2e
```

`playwright.config.ts` starts the fixture with the branch-pinned Dioxus CLI.
Set `SKIP_WEBSERVER=1` to target an already-running fixture and
`FIXTURE_PORT` to override port `18083`.
