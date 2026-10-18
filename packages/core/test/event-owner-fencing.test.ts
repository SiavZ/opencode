import { describe, expect } from "bun:test"
import { Effect, Exit, Layer } from "effect"
import { EventV2 } from "@opencode-ai/core/event"
import { Session } from "@opencode-ai/schema/session"
import { SessionV1 } from "@opencode-ai/schema/session-v1"
import { Database } from "@opencode-ai/core/database/database"
import { AppNodeBuilder } from "@opencode-ai/core/effect/app-node-builder"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { EventSequenceTable, EventTable } from "@opencode-ai/core/event/sql"
import { Location } from "@opencode-ai/core/location"
import { AbsolutePath } from "@opencode-ai/core/schema"
import { WorkspaceV2 } from "@opencode-ai/core/workspace"
import { eq } from "drizzle-orm"
import { location } from "./fixture/location"
import { testEffect } from "./lib/effect"

const locationLayer = Layer.succeed(
  Location.Service,
  Location.Service.of(
    location({ directory: AbsolutePath.make("project"), workspaceID: WorkspaceV2.ID.make("wrk_test") }),
  ),
)

const DurableMessage = SessionV1.Event.MessageRemoved
const durableData = (sessionID: Session.ID, text: string) => ({
  sessionID,
  messageID: SessionV1.MessageID.ascending(`msg_${text}`),
})

const it = testEffect(
  AppNodeBuilder.build(LayerNode.group([Database.node, EventV2.node, Location.node]), [[Location.node, locationLayer]]),
)

describe("EventV2 Owner Fencing Edge Cases", () => {
  it.effect("claim API on non-existent aggregate succeeds without creating a row", () =>
    Effect.gen(function* () {
      const events = yield* EventV2.Service
      const { db } = yield* Database.Service
      const aggregateID = Session.ID.create()

      yield* events.claim(aggregateID, "owner-a")

      const row = yield* db
        .select({ ownerID: EventSequenceTable.owner_id })
        .from(EventSequenceTable)
        .where(eq(EventSequenceTable.aggregate_id, aggregateID))
        .get()
        .pipe(Effect.orDie)

      expect(row).toBeUndefined()
    }),
  )

  it.effect("claim API is idempotent for the same owner", () =>
    Effect.gen(function* () {
      const events = yield* EventV2.Service
      const { db } = yield* Database.Service
      const aggregateID = Session.ID.create()

      yield* events.publish(DurableMessage, durableData(aggregateID, "seed"))
      yield* events.claim(aggregateID, "owner-a")
      yield* events.claim(aggregateID, "owner-a")

      const rows = yield* db
        .select({ seq: EventSequenceTable.seq, ownerID: EventSequenceTable.owner_id })
        .from(EventSequenceTable)
        .where(eq(EventSequenceTable.aggregate_id, aggregateID))
        .all()
        .pipe(Effect.orDie)

      expect(rows).toHaveLength(1)
      expect(rows[0]?.ownerID).toBe("owner-a")
    }),
  )

  it.effect("owner takeover during replay silently skips unstrict replay but defects on strict replay", () =>
    Effect.gen(function* () {
      const events = yield* EventV2.Service
      const { db } = yield* Database.Service
      const aggregateID = Session.ID.create()

      yield* events.publish(DurableMessage, durableData(aggregateID, "seed"))
      yield* events.claim(aggregateID, "owner-a")

      const received = new Array<EventV2.Payload>()
      yield* events.listen((event) => Effect.sync(() => received.push(event)))

      yield* events.replay(
        {
          id: EventV2.ID.create(),
          type: EventV2.versionedType(DurableMessage.type, 1),
          seq: 1,
          aggregateID,
          data: durableData(aggregateID, "ignored"),
        },
        { ownerID: "owner-b" },
      )

      expect(received).toHaveLength(0)

      const exit = yield* events
        .replay(
          {
            id: EventV2.ID.create(),
            type: EventV2.versionedType(DurableMessage.type, 1),
            seq: 1,
            aggregateID,
            data: durableData(aggregateID, "rejected"),
          },
          { ownerID: "owner-b", strictOwner: true },
        )
        .pipe(Effect.exit)

      expect(String(exit)).toContain("Replay owner mismatch")

      const rows = yield* db
        .select()
        .from(EventTable)
        .where(eq(EventTable.aggregate_id, aggregateID))
        .all()
        .pipe(Effect.orDie)

      expect(rows).toHaveLength(1)
    }),
  )

  it.effect("replayAll with strictOwner rejects on first owner mismatch", () =>
    Effect.gen(function* () {
      const events = yield* EventV2.Service
      const aggregateID = Session.ID.create()

      yield* events.publish(DurableMessage, durableData(aggregateID, "seed"))
      yield* events.claim(aggregateID, "owner-a")

      const exit = yield* events
        .replayAll(
          [
            {
              id: EventV2.ID.create(),
              type: EventV2.versionedType(DurableMessage.type, 1),
              seq: 1,
              aggregateID,
              data: durableData(aggregateID, "rejected"),
            },
          ],
          { ownerID: "owner-b", strictOwner: true },
        )
        .pipe(Effect.exit)

      expect(String(exit)).toContain("Replay owner mismatch")
    }),
  )

  it.effect("concurrent replays with different owners race to claim an unowned aggregate", () =>
    Effect.gen(function* () {
      const events = yield* EventV2.Service
      const { db } = yield* Database.Service
      const aggregateID = Session.ID.create()

      yield* events.publish(DurableMessage, durableData(aggregateID, "seed"))

      const idA = EventV2.ID.create()
      const idB = EventV2.ID.create()

      const replayA = events.replay(
        {
          id: idA,
          type: EventV2.versionedType(DurableMessage.type, 1),
          seq: 1,
          aggregateID,
          data: durableData(aggregateID, "replayA"),
        },
        { ownerID: "owner-a", strictOwner: true },
      )

      const replayB = events.replay(
        {
          id: idB,
          type: EventV2.versionedType(DurableMessage.type, 1),
          seq: 1,
          aggregateID,
          data: durableData(aggregateID, "replayB"),
        },
        { ownerID: "owner-b", strictOwner: true },
      )

      const [exitA, exitB] = yield* Effect.all([Effect.exit(replayA), Effect.exit(replayB)], {
        concurrency: "unbounded",
      })

      const success = Exit.isSuccess(exitA) ? exitA : Exit.isSuccess(exitB) ? exitB : null
      const failure = Exit.isFailure(exitA) ? exitA : Exit.isFailure(exitB) ? exitB : null

      expect(success).not.toBeNull()
      expect(failure).not.toBeNull()
      expect(String(failure)).toContain("Replay owner mismatch")

      const row = yield* db
        .select({ ownerID: EventSequenceTable.owner_id })
        .from(EventSequenceTable)
        .where(eq(EventSequenceTable.aggregate_id, aggregateID))
        .get()
        .pipe(Effect.orDie)

      const winnerOwner = Exit.isSuccess(exitA) ? "owner-a" : "owner-b"
      expect(row?.ownerID).toBe(winnerOwner)

      const rows = yield* db
        .select()
        .from(EventTable)
        .where(eq(EventTable.aggregate_id, aggregateID))
        .all()
        .pipe(Effect.orDie)

      expect(rows).toHaveLength(2)
    }),
  )

  it.effect("replayAll with ownerID claims an unowned aggregate and processes the whole batch", () =>
    Effect.gen(function* () {
      const events = yield* EventV2.Service
      const { db } = yield* Database.Service
      const aggregateID = Session.ID.create()
      const received = new Array<EventV2.Payload>()
      yield* events.listen((event) => Effect.sync(() => received.push(event)))

      const replayedEvents = [
        {
          id: EventV2.ID.create(),
          type: EventV2.versionedType(DurableMessage.type, 1),
          seq: 0,
          aggregateID,
          data: durableData(aggregateID, "one"),
        },
        {
          id: EventV2.ID.create(),
          type: EventV2.versionedType(DurableMessage.type, 1),
          seq: 1,
          aggregateID,
          data: durableData(aggregateID, "two"),
        },
      ]

      const source = yield* events.replayAll(replayedEvents, { ownerID: "owner-1", publish: true })

      expect(source).toBe(aggregateID)
      expect(received).toHaveLength(2)
      expect(received[0]).toMatchObject({
        id: replayedEvents[0].id,
        durable: { seq: 0, version: 1 },
        data: replayedEvents[0].data,
      })
      expect(received[1]).toMatchObject({
        id: replayedEvents[1].id,
        durable: { seq: 1, version: 1 },
        data: replayedEvents[1].data,
      })

      const sequence = yield* db
        .select({ seq: EventSequenceTable.seq, ownerID: EventSequenceTable.owner_id })
        .from(EventSequenceTable)
        .where(eq(EventSequenceTable.aggregate_id, aggregateID))
        .get()
        .pipe(Effect.orDie)

      expect(sequence).toEqual({ seq: 1, ownerID: "owner-1" })
    }),
  )
})
