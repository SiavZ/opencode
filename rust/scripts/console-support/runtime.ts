// Purpose: delegate unchanged support SSR lookup to the native operator API.
// Scope: server-only, loopback-only read transport. No deployment or identity-provider policy.
// Invariants: credentials come only from server environment and never from callers or build constants.
// Contract tests: runtime.test.ts and acceptance.ts.
export function configuration(env = process.env) {
  const raw = env.OPENCODE_SUPPORT_RUST_URL
  const secret = env.SUPPORT_API_KEY
  if (!raw || !secret || secret.trim() !== secret || /[\r\n]/.test(secret)) throw new Error("Support bridge configuration missing or invalid")
  const backend = new URL(raw)
  if (backend.protocol !== "http:" || !["127.0.0.1", "[::1]"].includes(backend.hostname) || backend.pathname !== "/" || backend.username || backend.password || backend.search || backend.hash) throw new Error("Support backend must be a literal loopback HTTP origin")
  const port = Number(env.OPENCODE_SUPPORT_PORT ?? 3031)
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("Support port invalid")
  return { backend, secret, origin: `http://127.0.0.1:${port}`, port }
}

export function allowed(request: Request, origin: string) {
  const url = new URL(request.url)
  if (url.origin !== origin || request.headers.get("host") !== new URL(origin).host) return false
  const source = request.headers.get("origin")
  if (source && source !== origin) return false
  const site = request.headers.get("sec-fetch-site")
  return !site || site === "same-origin" || site === "none"
}

export async function lookup(identifier: string) {
  if (typeof window !== "undefined") throw new Error("Support lookup is server-only")
  const config = configuration()
  if (typeof identifier !== "string" || !identifier.trim()) throw new Error("Identifier is required")
  const url = new URL("/api/support/lookup", config.backend)
  url.searchParams.set("identifier", identifier)
  const response = await fetch(url, {
    headers: { authorization: `Bearer ${config.secret}`, accept: "application/json" },
    redirect: "error", signal: AbortSignal.timeout(15_000),
  }).catch(() => { throw new Error("Native support lookup unavailable") })
  const reader = response.body?.getReader()
  if (!reader) throw new Error("Native support lookup unavailable")
  const chunks: Uint8Array[] = []
  let length = 0
  try {
    while (true) {
      const next = await reader.read()
      if (next.done) break
      length += next.value.length
      if (length > 4 * 1024 * 1024) throw new Error("Native support response too large")
      chunks.push(next.value)
    }
  } catch { throw new Error("Native support lookup unavailable") }
  finally { await reader.cancel().catch(() => {}) }
  const bytes = new Uint8Array(length)
  let offset = 0
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length }
  // Status errors never parse or reflect the backend response body.
  if (!response.ok) {
    if (response.status === 404) throw new Error("Support identifier not found")
    if (response.status === 400) throw new Error("Support identifier invalid")
    throw new Error("Native support lookup unavailable")
  }
  const value = await Promise.resolve().then(() => JSON.parse(new TextDecoder().decode(bytes))).catch(() => { throw new Error("Native support response invalid") })
  if (!value || typeof value !== "object" || value.identifier !== identifier || !Array.isArray(value.workspaces)) throw new Error("Native support response invalid")
  // A misconfigured backend must not accidentally echo this process's privileged credential.
  if (JSON.stringify(value).includes(config.secret)) throw new Error("Native support response invalid")
  return value
}