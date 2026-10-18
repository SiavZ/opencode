// Purpose: drive the real TypeScript credential, saved-permission and project-directory services for the
// Rust repository interop test (rust/crates/storage-sqlite/tests/repositories_interop.rs).
// Scope: test harness. Run from packages/core with HOME/XDG and OPENCODE_DB pointed at temp paths.
// Uses: packages/core/src/{credential.ts,permission/saved.ts,project/directories.ts,database/database.ts}
// Invariants: mode `read` prints {credentials, permissions, directories} as JSON for project `prj_rust`.
//   Mode `write` creates rows through the TS services for project `prj_ts` and prints nothing else.
import { Effect } from "../../../../../packages/core/node_modules/effect/dist/index.js"
import { Credential } from "../../../../../packages/core/src/credential"
import { LayerNode } from "../../../../../packages/core/src/effect/layer-node"
import { PermissionSaved } from "../../../../../packages/core/src/permission/saved"
import { ProjectDirectories } from "../../../../../packages/core/src/project/directories"

const mode = process.argv[2]
const layer = LayerNode.compile(LayerNode.group([Credential.node, PermissionSaved.node, ProjectDirectories.node]))

const program = Effect.gen(function* () {
  const credentials = yield* Credential.Service
  const permissions = yield* PermissionSaved.Service
  const directories = yield* ProjectDirectories.Service
  if (mode === "write") {
    yield* credentials.create({
      integrationID: "anthropic" as never,
      label: "From TS",
      value: { type: "oauth", methodID: "device", refresh: "r", access: "a", expires: 7 } as never,
    })
    yield* permissions.add({ projectID: "prj_ts" as never, action: "bash", resources: ["git *", "ls"] })
    yield* directories.create({ projectID: "prj_ts" as never, directory: "/ts/dir" as never, strategy: "s" })
    console.log(JSON.stringify({ ok: true }))
    return
  }
  console.log(
    JSON.stringify({
      credentials: (yield* credentials.all()).map((c) => ({ ...c })),
      permissions: (yield* permissions.list({ projectID: "prj_rust" as never })).map((p) => ({ ...p })),
      directories: yield* directories.list("prj_rust" as never),
    }),
  )
})

await Effect.runPromise(program.pipe(Effect.provide(layer as never), Effect.scoped) as never)
