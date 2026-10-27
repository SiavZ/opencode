# Local support SSR

Build the unchanged support UI and one exact server-action rewrite:

```sh
bun rust/scripts/console-support/build.ts
```

This writes only under `rust/target/console-support`. It does not need
`SUPPORT_API_KEY`, does not edit `packages/`, does not install dependencies and
is not a public deployment. The lookup page keeps its SolidStart query key and
result renderer. The compiler transform checks the precise current `getLookup`
AST and removes the direct database/resource lookup import. Any baseline shape
drift fails the build.

Final real-native Chromium acceptance is owned by the Rust support fixture
harness. It provides a disposable migrated MySQL fixture and these environment
values to `acceptance.ts`:

- `OPENCODE_SUPPORT_RUST_URL=http://127.0.0.1:<ephemeral-port>/`
- `OPENCODE_SUPPORT_PORT=<free-loopback-port>`
- `SUPPORT_API_KEY=<fixture-only canary>`
- `OPENCODE_SUPPORT_TEST_IDENTIFIER=<fixture workspace identifier>`
- `OPENCODE_SUPPORT_TEST_WORKSPACE=<fixture workspace id>`
- `OPENCODE_SUPPORT_TEST_NAME=<distinctive fixture workspace title>`
- `OPENCODE_SUPPORT_TEST_EMAIL=<fixture auth email>`

Run after the Rust fixture starts:

```sh
bun rust/scripts/console-support/acceptance.ts
```

The script starts and joins the Node SSR/UI processes, opens actual headless
Chromium, submits the unchanged target-blank lookup form, checks the result
sections and native missing/wrong bearer behavior, rejects non-loopback
configuration, and scans browser traffic and every public/generated artifact
for the canary. The operator listener and the framework child bind only to
127.0.0.1, require the exact Host, reject hostile Origin and cross-site fetch
metadata, and expose no native API proxy. There is no TLS or public ingress
mode. Loopback means local-machine operator access, not authenticated
multi-user access; do not expose it beyond a trusted operator machine.

Fast isolated bridge checks, with no database or provider calls:

```sh
bun test rust/scripts/console-support/plugin.test.ts rust/scripts/console-support/runtime.test.ts
```

The runtime reads the bearer only when the server action executes. It sends a
fresh Authorization header to the fixed native lookup endpoint, does not copy
caller headers, rejects redirects, bounds response size and replaces backend
read/parse/auth failures with fixed messages. The key is absent from HTML,
client bundles and request arguments.
