// Purpose: verify actual runtime argument translation over a local HTTP capture boundary.
// Scope: transport contract tests, not a replacement for Rust business or Chromium acceptance.
// Uses: runtime.ts; installed Solid request context and router Response helpers.
// Invariants: original FormData and positional arguments produce exact Rust call shapes.
// Contract tests: this file.
import { afterAll, expect, plugin, test } from "bun:test"
import { createRequire } from "node:module"
import { resolve } from "node:path"
import { appRoot } from "./plugin"

const require = createRequire(resolve(appRoot, "package.json"))
const compiled = await Bun.build({ entrypoints: [resolve(import.meta.dir, "runtime.ts")], target: "bun", outdir: resolve(process.env.JCODE_SCRATCH_DIR ?? resolve(import.meta.dir, "../../target/console-browser"), `runtime-test-${process.pid}`),
  plugins: [{ name: "installed-solid-imports", setup(build) { build.onLoad({ filter: /runtime\.ts$/ }, async (args) => ({ loader: "ts", contents: (await Bun.file(args.path).text()).replace('from "solid-js/web"', `from ${JSON.stringify(require.resolve("solid-js/web"))}`).replace('from "@solidjs/router"', `from ${JSON.stringify(resolve(require.resolve("@solidjs/router"), "../data/response.js"))}`) })) } }],
  external: [require.resolve("solid-js/web"), resolve(require.resolve("@solidjs/router"), "../data/response.js")],
})
if (!compiled.success) throw new AggregateError(compiled.logs, "Runtime contract compile failed")
const { provideRequestEvent } = await import(require.resolve("solid-js/web/storage"))
const { invoke } = await import(compiled.outputs[0].path)
const captures: Record<string, unknown>[] = []
const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: async (request) => {
  captures.push(await request.json())
  return Response.json({ id: "wrk_created", isAdmin: true, isBeta: true, actorID: "usr_fixture", actorRole: "admin", data: true, releases: [] }, { headers: { "set-cookie": "auth=updated; Path=/; HttpOnly" } })
} })
const oldBackend = process.env.OPENCODE_BROWSER_RUST_URL
const oldOrigin = process.env.OPENCODE_CONSOLE_PUBLIC_ORIGIN
process.env.OPENCODE_BROWSER_RUST_URL = server.url.origin
process.env.OPENCODE_CONSOLE_PUBLIC_ORIGIN = "http://127.0.0.1:3001"
afterAll(() => { server.stop(true); if (oldBackend === undefined) delete process.env.OPENCODE_BROWSER_RUST_URL; else process.env.OPENCODE_BROWSER_RUST_URL = oldBackend; if (oldOrigin === undefined) delete process.env.OPENCODE_CONSOLE_PUBLIC_ORIGIN; else process.env.OPENCODE_CONSOLE_PUBLIC_ORIGIN = oldOrigin })

async function call(operation: string, args: unknown[]) {
  const event = { request: new Request("http://127.0.0.1:3001/_server", { method: "POST", headers: { cookie: "auth=fixture", origin: "http://127.0.0.1:3001" } }), response: { headers: new Headers() }, locals: {} }
  const start = captures.length
  const result = await provideRequestEvent(event, () => invoke(operation, args))
  if (captures.length > start) expect(event.response.headers.get("set-cookie")).toContain("auth=updated")
  else expect(event.response.headers.get("set-cookie")).toBeNull()
  return result
}

test("every positional public operation sends its exact Rust argument envelope", async () => {
  const cases: [string, unknown[], Record<string, unknown>][] = [
    ...["userEmail", "session.get", "workspace.get", "key.list", "member.list", "provider.list", "model.info", "billing.get", "payment.list", "subscription.get", "lite.subscription.get", "cancelWaitlist"].map((operation): [string, unknown[], Record<string, unknown>] => [operation, ["wrk_fixture"], { operation, workspace: "wrk_fixture" }]),
    ...["workspaces", "black.paused", "lastWorkspace", "getBenchmarks", "github", "changelog", "auth.logout"].map((operation): [string, unknown[], Record<string, unknown>] => [operation, [], { operation }]),
    ["checkLoggedIn.get", [], { operation: "lastWorkspace" }],
    ["go.referral.capture", [], { operation: "go.referral.capture", input: {} }],
    ["usage.list", ["wrk_fixture", 2], { operation: "usage.list", workspace: "wrk_fixture", page: 2 }],
    ["checkoutUrl", ["wrk_fixture", 20, "http://127.0.0.1:3001/success", "http://127.0.0.1:3001/cancel"], { operation: "checkoutUrl", workspace: "wrk_fixture", input: { amount: 20, successUrl: "http://127.0.0.1:3001/success", cancelUrl: "http://127.0.0.1:3001/cancel" } }],
    ...["sessionUrl", "liteSessionUrl"].map((operation): [string, unknown[], Record<string, unknown>] => [operation, ["wrk_fixture", "http://127.0.0.1:3001/return"], { operation, workspace: "wrk_fixture", input: { returnUrl: "http://127.0.0.1:3001/return" } }]),
    ["receipt.download", ["wrk_fixture", "pay_fixture"], { operation: "receipt.download", workspace: "wrk_fixture", input: { paymentID: "pay_fixture" } }],
    ["enroll", ["wrk_fixture"], { operation: "enroll", workspace: "wrk_fixture", input: {} }],
    ["liteCheckoutUrl", ["wrk_fixture", "success", "cancel", "upi"], { operation: "liteCheckoutUrl", workspace: "wrk_fixture", input: { successUrl: "success", cancelUrl: "cancel", method: "upi" } }],
    ["lite.subscription.usage", ["wrk_fixture", "weekly"], { operation: "lite.subscription.usage", workspace: "wrk_fixture", input: { window: "weekly" } }],
    ...["go.referral.usagePreview", "go.referral.reward.apply"].map((operation): [string, unknown[], Record<string, unknown>] => [operation, ["wrk_fixture", "ref_fixture"], { operation, workspace: "wrk_fixture", input: { referralID: "ref_fixture" } }]),
    ["go.referral.get", ["wrk_fixture"], { operation: "go.referral.get", workspace: "wrk_fixture" }],
    ["getCosts", ["wrk_fixture", 2026, 9, "+02:00"], { operation: "getCosts", workspace: "wrk_fixture", input: { year: 2026, month: 9, tzOffset: "+02:00" } }],
    ["getTaskDetail", ["bench_fixture", "task/a"], { operation: "getTaskDetail", workspace: "bench_fixture", resource: "task/a" }],
  ]
  for (const [operation, args, expected] of cases) {
    const start = captures.length
    await call(operation, args)
    expect(captures[start]).toEqual(expected)
  }
})

test("all baseline forms preserve fields, trimming, toggles and cache revalidation", async () => {
  const cases: [string, Record<string, string>, Record<string, unknown>, string | undefined][] = [
    ["createWorkspace", { workspaceName: " New " }, { operation: "workspace.create", input: { name: "New" } }, undefined],
    ["workspace.update", { name: " Renamed " }, { input: { name: "Renamed" } }, "workspace.get"],
    ["key.create", { name: " Key " }, { input: { name: "Key" } }, "key.list"],
    ["key.remove", { id: "key_fixture" }, { resource: "key_fixture" }, "key.list"],
    ["member.create", { email: " user@example.test ", role: "member", limit: "12" }, { input: { email: "user@example.test", role: "member", monthlyLimit: 12 } }, "member.list"],
    ["member.update", { id: "usr_fixture", role: "admin", limit: "" }, { resource: "usr_fixture", input: { role: "admin", monthlyLimit: null } }, "member.list"],
    ["member.remove", { id: "usr_fixture" }, { resource: "usr_fixture" }, "member.list"],
    ["provider.save", { provider: "openai", credentials: "sk-fixture" }, { resource: "openai", input: { provider: "openai", credentials: "sk-fixture" } }, "provider.list"],
    ["provider.remove", { provider: "openai" }, { resource: "openai", input: { provider: "openai" } }, "provider.list"],
    ["model.toggle", { model: "gpt-fixture", enabled: "true" }, { input: { model: "gpt-fixture", enabled: true } }, "model.info"],
    ["billing.reload", {}, {}, "billing.get"],
    ["billing.setReload", { reload: "true", reloadAmount: "20", reloadTrigger: "5" }, { input: { reload: true, reloadAmount: 20, reloadTrigger: 5 } }, "billing.get"],
    ["billing.setMonthlyLimit", { limit: "42" }, { input: { limit: 42 } }, "billing.get"],
    ["billing.redeemCoupon", { code: " gofreemonth " }, { input: { code: "GOFREEMONTH" } }, "billing.get"],
    ["setUseBalance", { useBalance: "true" }, { input: { useBalance: true } }, "billing.get,subscription.get"],
    ["setLiteUseBalance", { useBalance: "false" }, { input: { useBalance: false } }, "billing.get,lite.subscription.get"],
    ["go.providerRouting.set", { useChinaProviders: "true" }, { input: { region: ["us", "eu", "sg"] } }, "lite.subscription.get"],
    ["go.allowTraining.set", { allowTraining: "true" }, { input: { allow_training: true } }, "lite.subscription.get"],
    ["emailSignup", { email: "user@example.test" }, { input: { email: "user@example.test" } }, undefined],
  ]
  for (const [operation, fields, patch, revalidate] of cases) {
    const form = new FormData()
    if (!["createWorkspace", "emailSignup"].includes(operation)) form.set("workspaceID", "wrk_fixture")
    Object.entries(fields).forEach(([key, value]) => form.set(key, value))
    const start = captures.length
    const result = await call(operation, [form])
    expect(captures[start]).toEqual({ operation, ...(!["createWorkspace", "emailSignup"].includes(operation) ? { workspace: "wrk_fixture" } : {}), ...patch })
    if (revalidate) { expect(result).toBeInstanceOf(Response); expect(result.headers.get("x-revalidate")).toBe(revalidate) }
  }
})

test("validation errors and cross-origin rejection never dispatch a Rust mutation", async () => {
  const start = captures.length
  const form = new FormData()
  form.set("workspaceID", "wrk_fixture")
  expect(await call("provider.save", [form])).toEqual({ error: "error.providerRequired" })
  form.set("provider", "openai")
  expect(await call("provider.save", [form])).toEqual({ error: "error.apiKeyRequired" })
  const event = { request: new Request("http://127.0.0.1:3001/_server", { method: "POST", headers: { origin: "https://foreign.example" } }), response: { headers: new Headers() }, locals: {} }
  expect(provideRequestEvent(event, () => invoke("workspace.update", [form]))).rejects.toThrow("Cross-origin")
  expect(captures.length).toBe(start)
})