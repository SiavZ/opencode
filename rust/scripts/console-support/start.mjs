// Purpose: serve built support SSR on literal loopback for a local operator.
// Scope: no public deployment, TLS proxy or identity-provider configuration.
// Invariants: exact Host/origin, no native API proxy, key only in server child environment.
// Contract tests: runtime.test.ts and real Chromium acceptance.ts.
import http from "node:http"
import { spawn } from "node:child_process"
import { fileURLToPath } from "node:url"
import { once } from "node:events"

const close = (server) => new Promise(resolve => { server.closeAllConnections(); server.close(() => resolve()) })
const wait = (child, timeout) => new Promise(resolve => {
  if (child.exitCode !== null || child.signalCode !== null || !child.pid) return resolve(true)
  const done = () => { clearTimeout(timer); child.off("exit", done); resolve(true) }
  const timer = setTimeout(() => { child.off("exit", done); resolve(false) }, timeout)
  child.once("exit", done)
})

export async function start() {
  const port = Number(process.env.OPENCODE_SUPPORT_PORT ?? 3031)
  const backend = new URL(process.env.OPENCODE_SUPPORT_RUST_URL ?? "http://invalid")
  const secret = process.env.SUPPORT_API_KEY
  if (!secret || secret.trim() !== secret || /[\r\n]/.test(secret) || !Number.isInteger(port) || port < 1 || port > 65535 || backend.protocol !== "http:" || !["127.0.0.1", "[::1]"].includes(backend.hostname) || backend.pathname !== "/" || backend.username || backend.password || backend.search || backend.hash) throw new Error("Local support configuration invalid")
  const reservation = http.createServer()
  reservation.listen(0, "127.0.0.1")
  await once(reservation, "listening")
  const address = reservation.address()
  const ssrPort = Number(process.env.OPENCODE_SUPPORT_SSR_PORT ?? (typeof address === "object" && address ? address.port : 0))
  await close(reservation)
  if (!Number.isInteger(ssrPort) || ssrPort < 1 || ssrPort > 65535 || ssrPort === port) throw new Error("Local support ports invalid")
  const entry = fileURLToPath(new URL("../../target/console-support/output/server/index.mjs", import.meta.url))
  const child = spawn("node", [entry], { env: { PATH: process.env.PATH, OPENCODE_SUPPORT_RUST_URL: backend.origin, SUPPORT_API_KEY: secret, OPENCODE_SUPPORT_PORT: String(port), OPENCODE_SUPPORT_SSR_PORT: String(ssrPort), NITRO_HOST: "127.0.0.1", NITRO_PORT: String(ssrPort) }, stdio: ["ignore", "pipe", "pipe"] })
  // Never reflect framework logs, which could contain request data or credentials.
  let logLeak = false
  for (const stream of [child.stdout, child.stderr]) {
    let tail = ""
    stream.on("data", chunk => { const text = tail + chunk.toString(); if (text.includes(secret)) logLeak = true; tail = text.slice(-secret.length) })
  }
  let stopping
  let failed = false
  const origin = `http://127.0.0.1:${port}`
  const server = http.createServer((request, response) => {
    const deny = (status, message) => { response.writeHead(status, { "cache-control": "no-store" }); response.end(message) }
    if (request.headers.host !== `127.0.0.1:${port}`) return deny(421, "Local host required")
    if (!request.url?.startsWith("/") || request.url.startsWith("//") || request.url.includes("\\")) return deny(400, "Invalid request target")
    if (request.headers.origin && request.headers.origin !== origin) return deny(403, "Local operator request required")
    if (request.headers["sec-fetch-site"] && !["same-origin", "none"].includes(request.headers["sec-fetch-site"])) return deny(403, "Local operator request required")
    const pathname = new URL(request.url, origin).pathname
    if (!["GET", "HEAD", "POST"].includes(request.method) || !(pathname === "/" || pathname === "/lookup" || pathname === "/_server" || pathname.startsWith("/_build/"))) return deny(404, "Not found")
    const headers = { ...request.headers, host: `127.0.0.1:${ssrPort}`, origin }
    for (const name of Object.keys(headers)) if (["authorization", "cookie", "forwarded"].includes(name) || name.startsWith("x-forwarded-")) delete headers[name]
    const upstream = http.request({ hostname: "127.0.0.1", port: ssrPort, path: request.url, method: request.method, headers }, result => {
      const headers = { ...result.headers, "cache-control": "no-store", "referrer-policy": "no-referrer", "x-content-type-options": "nosniff" }
      delete headers["set-cookie"]
      if (headers.location) {
        const location = URL.parse(headers.location, origin)
        if (!location || ![origin, `http://127.0.0.1:${ssrPort}`].includes(location.origin) || location.username || location.password) { result.resume(); return deny(502, "Support redirect rejected") }
        headers.location = `${location.pathname}${location.search}${location.hash}`
      }
      response.writeHead(result.statusCode ?? 502, headers)
      result.on("error", () => response.destroy())
      result.pipe(response)
    })
    upstream.setTimeout(20_000, () => upstream.destroy())
    upstream.on("error", () => { if (!response.headersSent) deny(502, "Support SSR unavailable"); else response.destroy() })
    request.on("aborted", () => upstream.destroy())
    response.on("close", () => { if (!response.writableEnded) upstream.destroy() })
    request.pipe(upstream)
  })
  const stop = () => stopping ??= (async () => {
    process.off("SIGTERM", terminate); process.off("SIGINT", terminate)
    await close(server)
    if (child.pid && child.exitCode === null && child.signalCode === null) {
      child.kill("SIGTERM")
      if (!await wait(child, 2000)) { child.kill("SIGKILL"); if (!await wait(child, 1000)) throw new Error("Support SSR child did not exit") }
    }
  })()
  const terminate = () => void stop().then(() => process.exit(0), () => process.exit(1))
  child.once("error", () => { failed = true; void stop() })
  child.once("exit", () => { failed = true; if (!stopping) void stop() })
  server.on("error", () => { failed = true; void stop() })
  // Imported acceptance owns process signals so it can also join Chromium.
  if (process.argv[1] === fileURLToPath(import.meta.url)) { process.once("SIGTERM", terminate); process.once("SIGINT", terminate) }
  try {
    const childUrl = `http://127.0.0.1:${ssrPort}/`
    const readyBy = Date.now() + 5000
    let ready = false
    while (Date.now() < readyBy && !failed) {
      ready = await fetch(childUrl, { headers: { origin }, redirect: "error", signal: AbortSignal.timeout(500) }).then(async response => { await response.body?.cancel(); return response.status === 200 }).catch(() => false)
      if (ready) break
      await new Promise(resolve => setTimeout(resolve, 50))
    }
    if (!ready || failed) throw new Error("Support SSR child failed to start")
    server.listen(port, "127.0.0.1")
    await once(server, "listening")
    if (failed || stopping) throw new Error("Support operator listener unavailable")
  } catch { await stop(); throw new Error("Support startup failed") }
  console.log("SUPPORT_OPERATOR_READY")
  return { stop, server, ssrPort, assertLogsClean() { if (logLeak) throw new Error("Support canary appeared in server logs") } }
}
if (process.argv[1] === fileURLToPath(import.meta.url)) start().catch(() => { console.error("Support startup failed"); process.exit(1) })