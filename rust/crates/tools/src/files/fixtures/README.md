# File-tool fixture provenance

Verified against the working TypeScript baseline on 2026-10-03.

- `core-schemas.json`: `packages/core/src/tool/tool.ts`'s exact `Schema.toJsonSchemaDocument(schema)` conversion, with `$defs` attached when definitions exist. Input/output exports are from `tool/{edit,write,apply-patch}.ts`. Read's private `LocationInput`, `Input`, and `Output` declarations were evaluated with their real `Schema`, `ReadToolFileSystem`, and `FileSystem` bindings from the baseline modules. Core schemas intentionally retain optional null unions, reject additional properties in the advertised schema, and preserve integer page constraints.
- `legacy-schemas.json`: all four baseline `export const Parameters = Schema.Struct(...)` declarations evaluated with real `Schema` and `NonNegativeInt`, then converted with the actual `packages/opencode/src/tool/json-schema.ts::fromSchema` adapter. All four JSON values matched the adopted fixture exactly, including key order.
- `mime-types.json`: verified by the read-tool node on 2026-10-04: deep-equal to `require("mime-types").types` from `mime-types@3.0.2` as resolved from `packages/core` (1239 extensions, zero differing keys). `read_common::mime_type` reproduces `lookup` (`extname("x." + path)`, lowercase, `application/octet-stream` fallback). Spot checks matched live baseline: `.ts` video/mp2t, `.rs` application/rls-services+xml, `.fbs` image/vnd.fastbidsheet, `.tsx`/`.py` octet-stream.
- `../descriptions/*.txt`: copied legacy descriptions. `files_contract.rs` compares every byte against `packages/opencode/src/tool/*.txt` on every test run. Core descriptions are literals in TypeScript, checked against those baseline source files.

The interrupted core fixture was incorrect: it used additionalProperties true, lost positive-integer read pagination constraints, and double-encoded numeric schemas. It was replaced with actual baseline producer output, not accepted on trust.

Baseline commands ran from the relevant package directory with HOME and XDG paths pointing at agent scratch directories. They performed no model calls, service execution, or writes to real user data.
