// Purpose: publish durable events through the real TypeScript EventV2 service so Rust can compare
// the stored `event` and `event_sequence` rows byte-for-byte.
// Scope: test harness invoked by event-store/tests/rows.rs with a temp HOME/XDG. Never touches real user data.
// Uses: packages/core/src/{event.ts,database/database.ts}
// Invariants: usage `bun publish_events.ts <dbfile> (<event-id> <json-data>)...`; prints the stored rows as JSON.
import { Effect, Layer, Schema } from "../../../../../packages/core/node_modules/effect/dist/index.js"
import { Database } from "../../../../../packages/core/src/database/database"
import { EventV2 } from "../../../../../packages/core/src/event"

const [file, ...payloads] = process.argv.slice(2)

const Sync = EventV2.define({
  type: "test.sync",
  durable: { version: 1, aggregate: "id" },
  schema: { id: Schema.String, text: Schema.String, nested: Schema.optional(Schema.Unknown) },
})

const program = Effect.gen(function* () {
  const events = yield* EventV2.Service
  const { db } = yield* Database.Service
  for (let index = 0; index < payloads.length; index += 2) {
    yield* events.publish(Sync, JSON.parse(payloads[index + 1]!), { id: payloads[index] as EventV2.ID })
  }
  const rows = yield* db.all<Record<string, unknown>>(
    "SELECT e.id, e.aggregate_id, e.seq, e.type, e.data, s.seq AS latest, s.owner_id FROM event e JOIN event_sequence s ON s.aggregate_id = e.aggregate_id ORDER BY e.aggregate_id, e.seq",
  )
  console.log(JSON.stringify(rows))
})

const database = Database.layerFromPath(file!)
await Effect.runPromise(
  program.pipe(Effect.provide(Layer.provideMerge(EventV2.layerWith(), database)), Effect.scoped),
)
