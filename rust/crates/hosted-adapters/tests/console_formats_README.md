# Zen provider format fixtures

Captured locally on 2026-10-27 from the actual TypeScript modules under
`packages/console/app/src/routes/zen/util/provider/`, without editing the baseline
or calling any provider. Bun ran from `packages/console/app` with HOME and all XDG
user-data directories redirected into a temporary scratch directory.

- `console_formats_fixtures.json`: 120 outputs from actual
  `createBodyConverter`, `createResponseConverter`, and
  `createStreamPartConverter` calls across every pair of Anthropic, OpenAI
  Responses, and OpenAI-compatible chat-completions formats. One request fixture
  records the baseline exception for flat function metadata passed to the
  Anthropic encoder; Rust returns `FormatError::InvalidRequest` instead.
- `console_formats_helpers.json`: 11 outputs from actual `anthropicHelper`,
  `openaiHelper`, `oaCompatHelper`, `googleHelper`, and `systemoneHelper` calls.
  Includes URL, headers, body overrides, normalized usage, and accumulated SSE
  usage, with Bedrock ARN/global model IDs and Databricks variants.
- `console_formats.rs`: invokes the public Rust adapters, compares those outputs,
  and separately tests byte fragmentation, Bedrock CRC/truncation handling,
  provider usage accounting, finish reasons, and opaque same-format payloads.

Only generated response message IDs and wall-clock `created` values are
normalized in comparisons. Tool IDs, arguments, optional/null distinctions,
finish reasons, cache data, and message ordering remain compared.

## Baseline compatibility details

`openai` means Responses API; `oa-compat` means chat completions. Same-format
payloads are opaque. The active legacy TypeScript handler rejects cross-format
provider routing and special Anthropic streaming body modifiers. Native routing
retains those restrictions even though the standalone baseline conversion
functions and Bedrock decoder are implemented and tested.

The baseline conversion functions have observable quirks, retained here:

- Chat-completions request output reads flat tool metadata even when common
  requests contain nested function metadata.
- Anthropic input normalization ignores string system/user content and accepts
  its typed block arrays; output defaults to 32,000 max tokens and applies at
  most four ephemeral cache markers in baseline visitation order.
- Responses output omits temperature and sets medium reasoning effort plus
  low verbosity, or medium verbosity for `gpt-5-codex`.
- Stream encoders emit the first common choice only. Anthropic stream output is
  raw JSON, not a synthesized SSE lifecycle. Malformed parts and terminal
  `[DONE]` input remain opaque, and usage-only parts can pass through unchanged.
- Google and Systemone have native helpers but no baseline cross-format
  conversion functions. Rust returns typed `Unsupported` for such conversions
  instead of an undefined payload.

The Bedrock decoder rejects invalid frame lengths, CRC errors, corrupt headers,
invalid base64/JSON, and a truncated final frame explicitly. This replaces the
baseline's log-and-stop behavior with a typed error rather than silently
accepting corrupt transport data.

## Scoped validation

From `rust/`, use the coordinated p12 target and mandated sccache environment:

```sh
export RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=3
export SCCACHE_BASEDIRS="$PWD/target" SCCACHE_CACHE_SIZE=100G
export CARGO_TARGET_DIR="$PWD/target/p12"
cargo test -p opencode-hosted-adapters --test console_formats
cargo clippy -p opencode-hosted-adapters --test console_formats -- -D warnings
```

Serialize Cargo jobs with the phase coordinator. Tests use offline fixtures,
not live providers. Full gateway routing and billing service acceptance belong
to the gateway owner's suites.