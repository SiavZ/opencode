# Phase 07: codemode (`opencode-codemode`)

Status: **implemented, all ported tests passing**. One open housekeeping item is listed under Remaining.

Baseline: `packages/codemode` (TS confined JS interpreter plus OpenAPI tool generation).
Crate: `rust/crates/codemode`. It is portable (`cargo check --target wasm32-unknown-unknown` passes) and uses no general JS engine. Parsing uses `oxc_parser`, and the crate interprets its own owned AST.

## Implemented

- Parse and lower (`parse/`): oxc parse, TypeScript erasure, an owned AST, and `Unsupported` nodes that produce diagnostics instead of panics.
- Interpreter (`interp/`): statements, expressions, closures, destructuring, async/await, Promises (all/allSettled/race/any), a task pool capped at 8 concurrent tool calls, race-loser interruption, a step budget, and cancellation ticks.
- Values (`value/`): an `Arc<Mutex>` object graph and copy-in/copy-out at the host boundary. Blocked keys are `__proto__`, `constructor` and `prototype`. The depth limit is 32. Cycles and un-awaited promises are rejected.
- Stdlib (`stdlib/`): Math, Number, String, RegExp (`regress`, UTF-16), Date, JSON, Object, Array, Map/Set, URL/URLSearchParams, and console.
- Tools (`tools/`): `Definition`/`ToolTree`, the `ToolRun` trait, `ToolError::{Refused, Internal, Interrupted}`, a validating `Schema`, TypeScript signature rendering, `$codemode.search` discovery, and catalog/instructions with a budget.
- Run (`run.rs`): `Runtime`, `execute`, `ExecutionLimits`, timeout plus cancellation via `select_biased`, and output bounding (`output.rs`).
- OpenAPI (`openapi/`): `from_spec` produces one tool per operation from a 3.0/3.1 document.
  - Inputs: path/query/header params, form/simple/deepObject styles, flattened or nested JSON bodies, and cross-location collision prefixes.
  - Specs: `$ref` chains, component `$defs`, and 3.0 normalization (nullable, example, boolean exclusive bounds; Effect `fromSchemaOpenApi3_0/3_1`).
  - Servers: operation, then path, then document precedence, plus base URL validation.
  - Security: requirements, schemes and auth carriers, with credentials resolved by the host `AuthResolver`.
  - Skipped operations: WebSocket, SSE, binary, cookie and malformed cases, each with a reason.
  - Responses: 50 MiB limit, malformed-JSON detection, and safe HTTP error summaries.
  - Transport goes only through the explicit host capability `HttpClient`. Without it, tools refuse to run.

## Tests (all passing, `cargo test -p opencode-codemode`)

| Suite | Baseline | Rust tests |
|---|---|---|
| parity | test/parity*.test.ts | 53 |
| stdlib | test/stdlib*.test.ts | 68 |
| promise | test/promise*.test.ts | 35 |
| enumeration | test/enumeration.test.ts | 14 |
| signature | test/signature.test.ts | 24 |
| codemode | test/codemode.test.ts | 43 |
| openapi | test/openapi.test.ts (real fixtures `openapi-happy-path.json`, `opencode-v2-openapi.json`) | 27 |
| unit | n/a | 1 |
| **total** | | **265** |

Adapted cases:
- The TS BigInt-default signature case cannot be represented in JSON.
- In the OpenAPI tests, TS inputs using `undefined` or a cyclic body were ported to the nearest JSON input that hits the same rejection path. Comments in `tests/openapi.rs` mark these.
- Effect host-failure shapes map to `ToolError` variants.

Validation run on 2026-10-03:
- `cargo clippy -p opencode-codemode --all-targets`: clean
- `cargo fmt`: clean
- `cargo check -p opencode-codemode --target wasm32-unknown-unknown`: ok

## Blockers / known divergences

| Area | Divergence | Owner/next |
|---|---|---|
| wasm timers | No clock or timer on wasm32. `ExecutionLimits.timeout` is enforced only on native (tokio). wasm hosts must cancel via `CancellationToken`. | host integration |
| Date | Local time is treated as UTC (no tz database). | accepted |
| `localeCompare` | Approximates ICU collation. | accepted |
| OpenAPI transport | No built-in HTTP client. The host must supply `HttpClient` (native reqwest or a wasm fetch adapter). | host integration |

## Remaining

- Six test files exceed 500 lines: `tests/codemode.rs` (1375), `openapi.rs` (1235), `stdlib.rs` (1030), `parity.rs` (664), `signature.rs` (603) and `promise.rs` (602).
  - Each is a 1:1 port of one baseline suite.
  - They need either a split into sibling test files or a modularity entry in `rust/contracts/modularity-reviews.toml` (owned by Phase 01).
  - All `src/` files are under 500 lines.

## Inventory (baseline to Rust)

| Baseline | Rust |
|---|---|
| src/codemode.ts | run.rs, lib.rs |
| src/interpreter*.ts, src/parse*.ts | parse/{mod,expr}.rs, ast.rs, interp/* |
| src/values*.ts, copyIn/copyOut | value/{mod,object,number,text,convert}.rs |
| src/stdlib/* | stdlib/* |
| src/tool.ts, tool-schema.ts | tools/{mod,schema,render,text}.rs |
| src/tool-runtime.ts | tools/{runtime,discovery}.rs |
| src/output.ts | output.rs |
| src/openapi/index.ts, types.ts | openapi/mod.rs |
| src/openapi/spec.ts | openapi/{spec,responses,naming,security,schema}.rs |
| src/openapi/runtime.ts | openapi/runtime.rs |
