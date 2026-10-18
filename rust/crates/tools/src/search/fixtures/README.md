# Search-tool fixture provenance

Captured from the working TypeScript baseline on 2026-10-04 by p05.tools-exec.

- `core-schemas.json`: `Schema.toJsonSchemaDocument` output (with `$defs` attached when definitions exist, exactly as `packages/core/src/tool/tool.ts::toJsonSchema`) for `GlobTool.Input/Output` and `GrepTool.Input/Output` exported from `packages/core/src/tool/{glob,grep}.ts`.
- `legacy-schemas.json`: `packages/opencode/src/tool/json-schema.ts::fromSchema` applied to the exported `Parameters` of `packages/opencode/src/tool/{glob,grep}.ts`.
- `../descriptions/{glob,grep}.txt`: byte copies of `packages/opencode/src/tool/{glob,grep}.txt`, compared byte for byte by `tools/tests/search_tools.rs` on every run. Core descriptions are literals checked against the baseline `.ts` sources.

Capture scripts live outside the repository (agent scratch). They ran from the package directories with HOME and XDG paths pointing at a scratch directory, made no model calls, and wrote no user data.
