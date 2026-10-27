// Purpose: verify production ingress route ownership and build configuration safety.
// Scope: local pure build/route tests, no network or deployment.
// Uses: production-start.mjs and build.ts source.
// Invariants: native endpoints never fall through to baseline cloud handlers.
// Contract tests: this file.
import { expect,test } from "bun:test"
import { isNative } from "./production-start.mjs"

test("production proxy routes native and localized APIs without claiming UI pages",()=>{
  for(const path of ["/auth","/api/email-signup","/stripe/webhook","/honeycomb/webhook","/zen/v1/chat/completions","/fr/api/enterprise","/docs/intro","/github.json","/_console/browser"]) expect(isNative(path)).toBe(true)
  for(const path of ["/","/workspace/wrk_example/settings","/go","/fr/go","/enterprise","/_build/assets/app.js"]) expect(isNative(path)).toBe(false)
})

test("production build uses Rust transforms and node preset outside baseline",async()=>{
  const source=await Bun.file(new URL("./build.ts",import.meta.url)).text()
  expect(source).toContain("rustConsole()")
  expect(source).toContain('preset: "node-server"')
  expect(source).toContain('framework.resolve("vite")')
  expect(source).toContain("configFile: false")
  expect(source).not.toContain('preset: "cloudflare-module"')
})
