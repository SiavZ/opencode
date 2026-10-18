// Purpose: drive the real TypeScript baseline database code for Rust interop tests.
// Scope: test harness invoked by storage-sqlite/tests/ts_interop.rs with a temp HOME/XDG. Never touches real user data.
// Uses: packages/core/src/database/{database.ts,migration.ts,migration.gen.ts,sqlite.bun.ts}
// Invariants: modes
//   open <file> [sql]        Database.layerFromPath (baseline PRAGMAs + DatabaseMigration.apply), then run sql, print rows JSON.
//   partial <file> <count>   apply only the first <count> registry migrations with DatabaseMigration.applyOnly.
import { Effect, Layer } from "../../../../../packages/core/node_modules/effect/dist/index.js"
import { EffectDrizzleSqlite } from "../../../../../packages/effect-drizzle-sqlite/src/index"
import { Database } from "../../../../../packages/core/src/database/database"
import { DatabaseMigration } from "../../../../../packages/core/src/database/migration"
import { migrations } from "../../../../../packages/core/src/database/migration.gen"
import { layer as sqliteLayer } from "../../../../../packages/core/src/database/sqlite.bun"

const [mode, file, arg] = process.argv.slice(2)

if (mode === "open") {
  const program = Effect.gen(function* () {
    const { db } = yield* Database.Service
    const rows = arg ? yield* db.all<Record<string, unknown>>(arg) : []
    console.log(JSON.stringify(rows))
  })
  await Effect.runPromise(program.pipe(Effect.provide(Database.layerFromPath(file!)), Effect.scoped))
}

if (mode === "partial") {
  const program = Effect.gen(function* () {
    const db = yield* EffectDrizzleSqlite.makeWithDefaults()
    yield* DatabaseMigration.applyOnly(db, migrations.slice(0, Number(arg)))
    console.log(JSON.stringify([]))
  })
  await Effect.runPromise(
    program.pipe(Effect.provide(sqliteLayer({ filename: file! }) as Layer.Layer<never>), Effect.scoped),
  )
}
