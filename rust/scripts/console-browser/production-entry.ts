// Purpose: wrap SolidStart SSR requests with the trusted public origin for colocated ingress.
// Scope: server-only production entry bundled by build.ts.
// Uses: baseline entry-server and runtime.ts server action invocations.
// Invariants: only the front proxy can reach this loopback listener; no caller-controlled origin trust.
// Contract tests: production.test.ts.
import handler from "../../../packages/console/app/src/entry-server"

export function fetch(request: Request) {
  const origin = process.env.OPENCODE_CONSOLE_PUBLIC_ORIGIN
  if (!origin) throw new Error("OPENCODE_CONSOLE_PUBLIC_ORIGIN is required")
  const source = new URL(request.url)
  const target = new URL(source.pathname+source.search,origin)
  return handler.fetch(new Request(target,request))
}
