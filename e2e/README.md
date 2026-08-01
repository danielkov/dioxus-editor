# Browser tests

Playwright drives the Dioxus fixture through real keyboard and browser input
events. The fixture exposes labeled, read-only model and selection dumps for
structural assertions.

```sh
cd e2e
npm ci
npx playwright install chromium
FIXTURE_PORT=18083 npm test
```

`playwright.config.ts` starts `../fixture` with `dx serve`. Set
`SKIP_WEBSERVER=1` only when using an already-running fixture locally.
Failures retain a Playwright trace; CI also produces an HTML report.
