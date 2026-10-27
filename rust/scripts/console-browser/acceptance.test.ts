// Purpose: keep unchanged frontend compiler operations aligned with Rust transport coverage.
// Scope: baseline-source compiler tests, no network or deployment.
// Uses: plugin.ts and the actual packages/console/app source tree.
// Invariants: no silently unsupported frontend action, no cloud imports in rewritten bodies.
// Contract tests: this file plus console::browser::tests live Chromium fixture.
import { expect, test } from "bun:test"
import { Glob } from "bun"
import { resolve } from "node:path"
import { appRoot, rewrite } from "./plugin"

const operations = [
  "userEmail", "auth.logout", "workspaces", "createWorkspace", "black.paused", "checkLoggedIn.get",
  "lastWorkspace", "session.get", "checkoutUrl", "billing.get", "provider.remove", "provider.save", "provider.list",
  "model.info", "model.toggle", "usage.list", "key.list", "sessionUrl", "billing.redeemCoupon", "subscription.get",
  "cancelWaitlist", "enroll", "setUseBalance", "billing.setMonthlyLimit", "payment.list", "receipt.download",
  "billing.reload", "billing.setReload", "member.list", "member.create", "member.remove", "member.update",
  "key.remove", "key.create", "getCosts", "lite.subscription.get", "lite.subscription.usage", "liteCheckoutUrl",
  "liteSessionUrl", "setLiteUseBalance", "go.providerRouting.set", "go.allowTraining.set", "workspace.get", "workspace.update",
  "getBenchmarks", "getTaskDetail", "github", "changelog", "go.referral.get", "go.referral.usagePreview",
  "go.referral.reward.apply", "go.referral.capture", "emailSignup",
]

test("every actual baseline server action maps to an explicit Rust operation", async () => {
  const found = new Set<string>()
  for await (const file of new Glob("**/*.{ts,tsx}").scan(resolve(appRoot, "src"))) {
    const path = resolve(appRoot, "src", file)
    const baseline = await Bun.file(path).text()
    const output = rewrite(baseline, path)?.code
    if (!output) continue
    for (const match of output.matchAll(/__rustConsoleAction\("([^"]+)"/g)) {
      // getActor is internal, removed from all rewritten actions. It is intentionally not public.
      if (match[1] === "getActor") { expect(file).toBe("context/auth.ts"); continue }
      expect(operations).toContain(match[1])
      found.add(match[1])
    }
    expect(await Bun.file(path).text()).toBe(baseline)
  }
  expect([...found].sort()).toEqual([...operations].sort())
})

test("rewritten settings retains real forms but removes database and resource imports", async () => {
  const path = resolve(appRoot, "src/routes/workspace/[id]/settings/settings-section.tsx")
  const output = rewrite(await Bun.file(path).text(), path)!.code
  expect(output).toContain('__rustConsoleAction("workspace.update", [form])')
  expect(output).toContain('"use server"')
  expect(output).toContain('action={updateWorkspace}')
  expect(output).not.toContain("console-core/")
  expect(output).not.toContain("Database")
})

test("referral middleware reuses actual baseline pure normalizer without cloud dependencies", async () => {
  const path = resolve(appRoot, "src/lib/referral-invite.ts")
  const output = rewrite(await Bun.file(path).text(), path)!.code
  expect(output).toContain("toUpperCase()")
  expect(output).toContain(".slice(0, 10)")
  expect(output).toContain('__rustConsoleAction("go.referral.capture", [])')
  expect(output).not.toContain("console-core/")
})