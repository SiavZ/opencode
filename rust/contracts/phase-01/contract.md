# Phase 01 contract: schema, fixtures and workspace tooling

Owner: Phase 01. Baseline: `packages/schema` at `907b3bc51` (effect 4.0.0-beta.83, bun 1.3.14).
Status of each item lives in `status.md`. Child work orders are in the swarm task graph (`p01.*`).

## `opencode-schema` (rust/crates/schema)

Portable crate (native and `wasm32-unknown-unknown`), no first-party dependencies. It owns stable
identifiers and shared values ported from `packages/schema/src` with byte-identical JSON. Phase 06
`protocol` owns wire DTOs and error envelopes built from these values.

### Wire rules (observed against the baseline with bun, see `values.rs` tests)

| Baseline construct | Rust | Encode | Decode |
| --- | --- | --- | --- |
| `Schema.String.pipe(Schema.brand(X))` | `brand!(Name)` newtype | bare string | any string |
| `...check(isStartsWith(p)).pipe(brand)` | `brand!(Name, "p")` | bare string | must start with `p`, error `Expected a string starting with "p", got "v"` |
| `optional(S)` (package helper) | `Option<T>` + `#[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "optional::deserialize")]` | key omitted when `None` | absent ok, explicit `null` rejected |
| raw `Schema.optional(S)` (V1 only) | `Option<T>` + `default, skip_serializing_if` | key omitted when `None` | absent ok, `null` rejected unless `NullOr` |
| `Schema.Finite` / `Schema.Number` | `Finite` | like `JSON.stringify`: integral values print without `.0`, `-0` prints `0`, integers above 2^53 print padded shortest digits | any finite JSON number, correct rounding (`float_roundtrip`) |
| `Schema.Int`, `NonNegativeInt`, `PositiveInt` | `Int`, `NonNegativeInt`, `PositiveInt` | integer | any JSON number that is a safe integer (`3.0` ok, `2^53` fails) |
| `DateTimeUtcFromMillis` | `DateTimeUtc` | epoch millis integer | finite number, truncated toward zero like `new Date(x)`, `abs <= 8.64e15` else `Invalid date` |
| `Schema.Record(String, Unknown)` / `Schema.Json` object | `JsonObject` (`serde_json::Map`, `preserve_order`) | insertion order | insertion order |
| `Schema.Literal("x")` single field | `literal!(Name = "x")` | `"x"` | must equal |
| `Schema.Literals([...])` | fieldless enum with renames | string | closed set |
| `Schema.toTaggedUnion("type")` | `#[serde(tag = "type")]` enum | tag first, then variant fields in order | unknown tag fails |
| Struct field order (with spreads) | struct field order | declaration order of the baseline Struct after spreading | unknown keys ignored |
| `Event.define({ type, durable, schema })` | data struct + `impl EventData { const DEFINITION }`, envelope `event::Event<D>` | `id, metadata?, type, durable?, location?, data` | `type` must match |

Number spelling was compared directly: `JSON.stringify([1.5e21, 1e-7, 1.5e-8, 0.1+0.2, 2**53, -0,
1e300, 5e-324, 123.456])` under bun 1.3.14 and `Finite` serialization both print
`[1.5e+21,1e-7,1.5e-8,0.30000000000000004,9007199254740992,0,1e+300,5e-324,123.456]`
(test `values::tests::number_spelling_matches_bun`).

### Module map

| Baseline | Rust module |
| --- | --- |
| `schema.ts` | `values` |
| `identifier.ts`, `core/src/id/id.ts` | `identifier` |
| `session-id.ts`, `workspace-id.ts`, `project-id.ts`, `integration-id.ts`, every `ID` brand | `ids` (V1 brands in `ids::v1`) |
| `location.ts` | `location` |
| `event.ts` | `event` |
| `prompt.ts`, `llm.ts` | `prompt` |
| `provider.ts`, `model.ts` | `provider` |
| `session.ts` | `session` |
| `session-message.ts` | `session_message` |
| `session-event.ts` | `session_event` |
| `session-input.ts`, `session-delivery.ts` | `session_input` |
| `revert.ts`, `file-diff.ts` | `revert` |
| `session-status-event.ts`, `session-compaction-event.ts`, `session-todo.ts` | `session_aux_event` |
| `agent.ts`, `command.ts`, `prompt-input.ts` | `agent` |
| `permission.ts`, `permission-saved.ts` | `permission` |
| `question.ts` | `question` |
| `pty.ts`, `pty-ticket.ts` | `pty` |
| `project.ts`, `project-copy.ts`, `project-directories.ts` | `project` |
| `workspace.ts`, `workspace-event.ts`, `worktree-event.ts` | `workspace` |
| `integration.ts`, `connection.ts`, `credential.ts` | `integration` |
| `filesystem.ts`, `filesystem-watcher.ts` | `filesystem` |
| `reference.ts`, `skill.ts` | `reference` |
| `catalog.ts`, `models-dev.ts`, `plugin.ts`, `*-event.ts` (ide, installation, lsp, mcp, server, tui, vcs) | `misc_event` |
| `event-manifest.ts`, `durable-event-manifest.ts` | `event_manifest` |
| `v1/*`, `session-v1.ts`, `permission-v1.ts`, `question-v1.ts`, `legacy-event.ts` | `v1` |

Change control: one shared-schema reviewer (Phase 01). Changes that affect other phases go through
Phase 90 with an impact list.

## xtask (rust/xtask)

`cargo xtask <task>`. Phase 01 owns dispatch in `src/main.rs`; other phases add one `mod` and one
arm. Library entry points live in `src/lib.rs` so tests drive the real code against temp trees.

| Task | Contract |
| --- | --- |
| `headers [--fix]` | Every `.rs` under `crates/`, `bins/`, `xtask/` has the `rust/AGENTS.md` header. `Uses`/`Used by` trees are regenerated from syn module/use resolution plus `contracts/runtime-edges.toml`. `--fix` is deterministic. |
| `edges` | Resolved Cargo graph (host and `wasm32-unknown-unknown`) only uses edges allowed by `contracts/crate-edges.toml`, every member is declared, wasm/portable crates never resolve native-only crates, combined graph is acyclic. |
| `size` | Files above 500 physical lines need an unexpired `[[review]]` in `contracts/modularity-reviews.toml` with an independent reviewer. |
| `inventory [--write]` | Extracts HTTP routes, tools, providers, CLI commands and events from the TS baseline into `contracts/phase-01/inventory.toml` with coverage IDs. Fails on unmatched or stale entries. |
| `fixtures [run]` | Validates versioned fixtures (`fixture-format.md`) under `contracts/fixtures/` and runs the Rust differential codecs against them byte-exactly. |
| `check` | Runs all of the above and reports every failure. |

## opencode-test-support (rust/crates/test-support)

Reusable conformance utilities (JSON byte equality, round trip, must-fail decode, temp HOME
sandbox, fixture loader). It never re-implements domain rules.
