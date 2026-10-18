// Purpose: generate storage-sqlite/src/migrations/registry.json from the TypeScript migration registry.
// Scope: build tooling. Runs every migration `up` against a recording fake transaction so the exact SQL
// strings are captured byte for byte. Conditional migrations record the probe SQL; the branch taken
// depends on the probe result and is ported by hand in src/migration.rs.
// Uses: packages/core/src/database/migration.gen.ts, packages/core/src/database/schema.gen.ts
// Invariants: output order equals the runtime registry order. Run from packages/core:
//   bun ../../rust/crates/storage-sqlite/scripts/extract_migrations.ts > ../../rust/crates/storage-sqlite/src/migrations/registry.json
import { Effect } from "../../../../packages/core/node_modules/effect/dist/index.js"
import { migrations } from "../../../../packages/core/src/database/migration.gen"
import schema from "../../../../packages/core/src/database/schema.gen"

// `probe` answers PRAGMA table_info probes. Recording with an empty answer captures the "column absent" branch.
function record(up: (tx: any) => any) {
  const statements: string[] = []
  const probes: string[] = []
  const tx = {
    run: (sql: unknown) => Effect.sync(() => void statements.push(String(sql))),
    all: (sql: unknown) => Effect.sync(() => (probes.push(String(sql)), [])),
  }
  Effect.runSync(up(tx))
  return { statements, probes }
}

console.log(
  JSON.stringify(
    {
      source: "packages/core/src/database/migration.gen.ts",
      schema: record(schema.up).statements,
      migrations: migrations.map((migration) => ({ id: migration.id, ...record(migration.up) })),
    },
    null,
    2,
  ),
)
