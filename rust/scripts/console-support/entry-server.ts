// Purpose: guard the unchanged support SSR handler behind local operator-only ingress.
// Invariants: runtime configuration required; hostile hosts/origins never execute server queries.
// Contract tests: acceptance.ts and runtime.test.ts.
import handler from "../../../packages/console/support/src/entry-server"
import { configuration } from "./runtime"
export function fetch(request: Request) {
  const config = configuration()
  const url = new URL(request.url)
  if ([...request.headers.keys()].some(name => name === "forwarded" || name.startsWith("x-forwarded-")) || request.headers.get("host") !== `127.0.0.1:${process.env.OPENCODE_SUPPORT_SSR_PORT}` || url.origin !== `http://127.0.0.1:${process.env.OPENCODE_SUPPORT_SSR_PORT}` || (request.headers.get("origin") && request.headers.get("origin") !== config.origin) || (request.headers.get("sec-fetch-site") && !["same-origin", "none"].includes(request.headers.get("sec-fetch-site")!))) return new Response("Local operator request required", { status: 403, headers: { "cache-control": "no-store" } })
  if (url.pathname !== "/" && url.pathname !== "/lookup" && url.pathname !== "/_server" && !url.pathname.startsWith("/_build/")) return new Response("Not found", { status: 404, headers: { "cache-control": "no-store" } })
  return handler.fetch(request)
}
