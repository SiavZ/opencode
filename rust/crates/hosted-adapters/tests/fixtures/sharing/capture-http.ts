// Purpose: capture HTTP fixtures (status, headers, bodies) from the real enterprise Hono API route
// (packages/enterprise/src/routes/api/[...path].ts) backed by an in-memory S3.
// Scope: fixture capture script, run manually: `cd packages/enterprise && bun <this file>`.
// Uses:
// `-- none
// External:
// `-- packages/enterprise/src/routes/api/[...path].ts
// Used by:
// rust/bins/
// `-- hosted/src/sharing/tests.rs
// Invariants: no network; share secrets are replaced by <secret>.
// Contract tests: bins/hosted/src/sharing/tests.rs
process.env.OPENCODE_STORAGE_ADAPTER = "r2"
process.env.OPENCODE_STORAGE_ACCOUNT_ID = "acct"
process.env.OPENCODE_STORAGE_ACCESS_KEY_ID = "AKIDEXAMPLE"
process.env.OPENCODE_STORAGE_SECRET_ACCESS_KEY = "secret"
process.env.OPENCODE_STORAGE_BUCKET = "bucket"
process.env.SST_RESOURCE_SUPPORT_API_KEY = JSON.stringify({ value: "support-key" })
process.env.SST_RESOURCE_App = JSON.stringify({ name: "opencode", stage: "test" })

const objects = new Map<string, string>()
const realFetch = globalThis.fetch
globalThis.fetch = (async (input: Request | string, init?: RequestInit) => {
  const req = input instanceof Request ? input : new Request(input, init)
  const url = new URL(req.url)
  if (!url.hostname.endsWith("r2.cloudflarestorage.com")) return realFetch(input, init)
  const key = decodeURIComponent(url.pathname.replace(/^\/bucket\/?/, ""))
  if (req.method === "GET" && url.searchParams.get("list-type") === "2") {
    const prefix = url.searchParams.get("prefix") ?? ""
    const keys = [...objects.keys()].filter((k) => k.startsWith(prefix)).sort()
    return new Response(`<ListBucketResult>${keys.map((k) => `<Key>${k}</Key>`).join("")}</ListBucketResult>`)
  }
  if (req.method === "GET") return objects.has(key) ? new Response(objects.get(key)) : new Response("", { status: 404 })
  if (req.method === "PUT") return objects.set(key, await req.text()), new Response("")
  if (req.method === "DELETE") return objects.delete(key), new Response("", { status: 204 })
  return new Response("", { status: 405 })
}) as typeof fetch

const route = await import(new URL("../../../../../../packages/enterprise/src/routes/api/[...path].ts", import.meta.url).href)
let secret = ""
const call = async (method: string, path: string, body?: unknown, headers: Record<string, string> = {}) => {
  const init: RequestInit = { method, headers: { ...(body === undefined ? {} : { "content-type": "application/json" }), ...headers } }
  if (body !== undefined) init.body = typeof body === "string" ? body : JSON.stringify(body)
  const request = new Request(`http://enterprise.local${path}`, init)
  const fn = route[method === "DELETE" ? "DELETE" : method === "POST" ? "POST" : "GET"]
  const res: Response = await fn({ request })
  const text = await res.text()
  const keep = ["content-type", "cache-control", "access-control-allow-origin"]
  return {
    method,
    path,
    request: body === undefined ? null : secret ? JSON.parse(JSON.stringify(body).split(secret).join("<secret>")) : body,
    requestHeaders: headers,
    status: res.status,
    headers: Object.fromEntries([...res.headers.entries()].filter(([k]) => keep.includes(k))),
    body: secret ? text.split(secret).join("<secret>") : text,
  }
}
const out = []
const created = await call("POST", "/api/share", { sessionID: "ses_0123456789abcdefGHIJ" }, { "x-forwarded-host": "opncd.ai" })
secret = JSON.parse(created.body).secret
created.body = created.body.split(secret).join("<secret>")
out.push(created)
out.push(await call("POST", "/api/share", { sessionID: "ses_0123456789abcdefGHIJ" }))
out.push(await call("POST", "/api/share", { nope: 1 }))
out.push(await call("POST", "/api/share", "not json"))
out.push(await call("POST", "/api/share/cdefGHIJ/sync", { secret, data: [{ type: "session", data: { id: "ses_x" } }] }))
out.push(await call("POST", "/api/share/cdefGHIJ/sync", { secret: "wrong", data: [] }))
out.push(await call("POST", "/api/share/cdefGHIJ/sync", { secret, data: [{ type: "bogus", data: 1 }] }))
out.push(await call("GET", "/api/share/cdefGHIJ/data"))
out.push(await call("GET", "/api/share/missing0/data"))
out.push(await call("DELETE", "/api/share/cdefGHIJ", { secret: "wrong" }))
out.push(await call("DELETE", "/api/support/actions/remove-share", { shareID: "cdefGHIJ" }, { authorization: "Bearer bad" }))
out.push(await call("DELETE", "/api/support/actions/remove-share", { shareID: "" }, { authorization: "Bearer support-key" }))
out.push(await call("DELETE", "/api/support/actions/remove-share", { shareID: "missing0" }, { authorization: "Bearer support-key" }))
out.push(await call("DELETE", "/api/support/actions/remove-share", { shareID: "cdefGHIJ" }, { authorization: "Bearer support-key" }))
out.push(await call("GET", "/api/share/cdefGHIJ/data"))
out.push(await call("GET", "/api/nothing"))
out.push(await call("POST", "/api/share", [1]))
out.push(await call("POST", "/api/share", { sessionID: 5 }))
out.push(await call("POST", "/api/share/x/sync", { data: "no" }))
out.push(await call("POST", "/api/share/x/sync", { secret: 1, data: [1, { type: "message" }, null, { type: 2 }] }))
out.push(await call("DELETE", "/api/support/actions/remove-share", "garbage", { authorization: "Bearer support-key" }))
out.push(await call("DELETE", "/api/support/actions/remove-share", { shareID: 3 }, { authorization: "Bearer support-key" }))
out.push(await call("DELETE", "/api/share/x", {}))
{
  const res = await route.GET({ request: new Request("http://enterprise.local/api/share", { method: "OPTIONS", headers: { origin: "https://a.example", "access-control-request-method": "POST", "access-control-request-headers": "content-type,x-custom" } }) })
  out.push({ method: "OPTIONS", path: "/api/share", request: null, requestHeaders: {}, status: res.status, headers: Object.fromEntries(res.headers.entries()), body: await res.text() })
}
{
  const res = await route.POST({ request: new Request("http://enterprise.local/api/share", { method: "POST", body: JSON.stringify({ sessionID: "ses_plain0001" }) }) })
  out.push({ method: "POST", path: "/api/share", request: "no content-type", requestHeaders: {}, status: res.status, headers: Object.fromEntries(res.headers.entries()), body: (await res.text()).replace(/"secret":"[^"]+"/, '"secret":"<secret>"') })
}
out.push(await call("GET", "/api/doc"))
console.log(JSON.stringify(out, null, 2))
