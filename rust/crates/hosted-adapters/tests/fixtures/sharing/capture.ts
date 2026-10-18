// Purpose: capture baseline fixtures for the Rust sharing port by running the real
// packages/enterprise Share/Storage modules against an in-memory S3 behind fetch interception.
// Scope: fixture capture script, run manually: `cd packages/enterprise && bun <this file>`.
// Uses:
// `-- none
// External:
// `-- packages/enterprise/src/core/{share,storage}.ts, aws4fetch
// Used by:
// rust/crates/
// `-- hosted-adapters/tests/sharing_baseline.rs
// Invariants: no network, fixed credentials, deterministic output (secret UUIDs are redacted).
// Contract tests: hosted-adapters/tests/sharing_baseline.rs
process.env.OPENCODE_STORAGE_ADAPTER = "r2"
process.env.OPENCODE_STORAGE_ACCOUNT_ID = "acct"
process.env.OPENCODE_STORAGE_ACCESS_KEY_ID = "AKIDEXAMPLE"
process.env.OPENCODE_STORAGE_SECRET_ACCESS_KEY = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY"
process.env.OPENCODE_STORAGE_BUCKET = "bucket"
process.env.NODE_ENV = "production"

const root = new URL("../../../../../../packages/enterprise/src/core/", import.meta.url)
const { Share } = await import(new URL("share.ts", root).href)
const { AwsClient } = await import(Bun.resolveSync("aws4fetch", root.pathname))

const objects = new Map<string, string>()
const trace: string[] = []
globalThis.fetch = (async (input: Request | string, init?: RequestInit) => {
  const req = input instanceof Request ? input : new Request(input, init)
  const url = new URL(req.url)
  const key = decodeURIComponent(url.pathname.replace(/^\/bucket\/?/, ""))
  if (req.method === "GET" && url.searchParams.get("list-type") === "2") {
    trace.push(`LIST ${url.searchParams.toString()}`)
    const prefix = url.searchParams.get("prefix") ?? ""
    const after = url.searchParams.get("start-after") ?? ""
    const max = Number(url.searchParams.get("max-keys") ?? "1000")
    const keys = [...objects.keys()].filter((k) => k.startsWith(prefix) && k > after).sort().slice(0, max)
    return new Response(`<ListBucketResult>${keys.map((k) => `<Contents><Key>${k}</Key></Contents>`).join("")}</ListBucketResult>`)
  }
  trace.push(`${req.method} ${key}`)
  if (req.method === "GET") return objects.has(key) ? new Response(objects.get(key)) : new Response("", { status: 404 })
  if (req.method === "PUT") {
    objects.set(key, await req.text())
    return new Response("")
  }
  if (req.method === "DELETE") {
    objects.delete(key)
    return new Response("", { status: 204 })
  }
  return new Response("", { status: 405 })
}) as typeof fetch

const redact = (s: string, secret: string) => s.split(secret).join("<secret>")
const out: Record<string, unknown> = {}

// 1. create + sync merge ordering (ICU collation via localeCompare).
const sessionID = "ses_0123456789abcdefGHIJ"
const share = await Share.create({ sessionID })
out.create = { id: share.id, sessionID: share.sessionID, secretLength: share.secret.length }
const p = (id: string, messageID: string, text: string) => ({ type: "part", data: { id, messageID, sessionID, type: "text", text } })
const m = (id: string) => ({ type: "message", data: { id, sessionID, role: "user" } })
const batch1 = [p("prt_b", "msg_B", "1"), m("msg_a"), m("msg_A"), { type: "session", data: { id: sessionID, title: "t" } }, p("prt_a", "msg_a_x", "2"), m("msg_a-"), m("msg_a_")]
const batch2 = [{ type: "model", data: [{ id: "m1" }] }, p("prt_b", "msg_B", "1-updated"), { type: "session_diff", data: [] }, m("msg_0")]
trace.length = 0
await Share.sync({ share: { id: share.id, secret: share.secret }, data: batch1 })
await Share.sync({ share: { id: share.id, secret: share.secret }, data: batch2 })
out.syncTrace = [...trace]
out.syncInputs = [batch1, batch2]
out.data = await Share.data(share.id)

// 2. legacy compaction import (no snapshot, events + compaction).
const legacyID = "legacy01"
objects.set(`share/${legacyID}.json`, JSON.stringify({ id: legacyID, secret: "s", sessionID: "ses_legacy01" }))
objects.set(`share_compaction/${legacyID}.json`, JSON.stringify({ event: "evt_002", data: [m("msg_1")] }))
objects.set(`share_event/${legacyID}/evt_001.json`, JSON.stringify([m("msg_0")]))
objects.set(`share_event/${legacyID}/evt_003.json`, JSON.stringify([p("prt_1", "msg_1", "x")]))
objects.set(`share_event/${legacyID}/evt_004.json`, JSON.stringify([m("msg_2")]))
trace.length = 0
out.legacyData = await Share.data(legacyID)
out.legacyTrace = [...trace]
out.legacyObjects = Object.fromEntries([...objects.entries()].filter(([k]) => k.includes(legacyID)).sort())

// 3. errors.
const errors: Record<string, string> = {}
for (const [name, run] of [
  ["duplicate", () => Share.create({ sessionID })],
  ["syncBadSecret", () => Share.sync({ share: { id: share.id, secret: "nope" }, data: [] })],
  ["syncMissing", () => Share.sync({ share: { id: "missing0", secret: "x" }, data: [] })],
  ["removeBadSecret", () => Share.remove({ id: share.id, secret: "nope" })],
] as const) {
  try {
    await run()
    errors[name] = "ok"
  } catch (e) {
    errors[name] = (e as Error).message
  }
}
out.errors = errors

// 4. remove trace.
objects.set(`share_data/${share.id}/session.json`, "{}")
trace.length = 0
await Share.remove({ id: share.id, secret: share.secret })
out.removeTrace = trace.map((t) => redact(t, share.secret))
out.remainingAfterRemove = [...objects.keys()].filter((k) => k.includes(share.id)).sort()

// 5. SigV4 vectors from aws4fetch (the signer the baseline uses).
const client = new AwsClient({ accessKeyId: "AKIDEXAMPLE", secretAccessKey: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY" })
const vectors = []
for (const [method, url, body] of [
  ["GET", "https://acct.r2.cloudflarestorage.com/bucket/share/abc.json", undefined],
  ["PUT", "https://acct.r2.cloudflarestorage.com/bucket/share_snapshot/abc.json", "{\"data\":[]}"],
  ["GET", "https://acct.r2.cloudflarestorage.com/bucket?list-type=2&prefix=share_event%2Fabc%2F&start-after=share_event%2Fabc%2Fevt_1.json", undefined],
  ["DELETE", "https://s3.us-east-1.amazonaws.com/bucket/a b/(x)!.json", undefined],
] as const) {
  const headers: Record<string, string> = body ? { "Content-Type": "application/json" } : {}
  const signed = await client.sign(url, { method, body, headers, aws: { datetime: "20261003T120000Z" } })
  vectors.push({ method, url, body: body ?? null, signedUrl: signed.url, authorization: signed.headers.get("authorization"), contentSha256: signed.headers.get("x-amz-content-sha256") })
}
out.sigv4 = vectors
console.log(JSON.stringify(out, null, 2))
