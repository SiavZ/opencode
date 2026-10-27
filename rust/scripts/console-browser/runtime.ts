// Purpose: translate baseline server-action arguments into Rust browser JSON calls.
// Scope: server execution only, with per-request cookie and redirect propagation.
// Uses: Solid request event; Solid router json and redirect; Rust /_console/browser.
// Invariants: loopback backend only; cross-origin requests rejected before mutation; no cloud fallback.
// Contract tests: acceptance.test.ts.
import { getRequestEvent } from "solid-js/web"
import { json, redirect } from "@solidjs/router"

function backend() {
  const url = new URL(process.env.OPENCODE_BROWSER_RUST_URL ?? "http://127.0.0.1:8787")
  if (url.protocol !== "http:" || !["127.0.0.1", "[::1]", "localhost"].includes(url.hostname) || url.username || url.password || url.pathname !== "/" || url.search || url.hash)
    throw new Error("OPENCODE_BROWSER_RUST_URL must be a loopback HTTP origin")
  return url
}

export async function invoke(operation: string, args: unknown[]) {
  const event = getRequestEvent()
  if (!event) throw new Error("Rust browser action requires a Solid request event")
  const requestOrigin = new URL(event.request.url).origin
  const expectedOrigin = process.env.OPENCODE_CONSOLE_PUBLIC_ORIGIN
  if (!expectedOrigin || requestOrigin !== new URL(expectedOrigin).origin) throw new Error("Rust browser public origin mismatch")
  const origin = event.request.headers.get("origin")
  const site = event.request.headers.get("sec-fetch-site")
  if ((origin && origin !== requestOrigin) || (site && !["same-origin", "none"].includes(site)))
    throw new Error("Cross-origin console action is not allowed")

  const form = args[0] instanceof FormData ? args[0] : undefined
  const field = (name: string) => { const value = form?.get(name); return typeof value === "string" ? value : undefined }
  const workspace = form ? field("workspaceID") : typeof args[0] === "string" ? args[0] : undefined
  const input: Record<string, unknown> = {}
  const call: { operation: string; workspace?: string; resource?: string; page?: number; input?: Record<string, unknown> } = { operation, workspace }
  const revalidate: Record<string, string | string[]> = {
    "key.create": "key.list", "key.remove": "key.list", "workspace.update": "workspace.get",
    "member.create": "member.list", "member.update": "member.list", "member.remove": "member.list",
    "provider.save": "provider.list", "provider.remove": "provider.list", "model.toggle": "model.info",
    "sessionUrl": "billing.get", "billing.reload": "billing.get", "billing.setReload": "billing.get",
    "billing.setMonthlyLimit": "billing.get", "billing.redeemCoupon": "billing.get",
    "cancelWaitlist": ["billing.get", "subscription.get"], "enroll": ["billing.get", "subscription.get"],
    "setUseBalance": ["billing.get", "subscription.get"], "setLiteUseBalance": ["billing.get", "lite.subscription.get"],
    "liteCheckoutUrl": ["billing.get", "lite.subscription.get"], "liteSessionUrl": ["billing.get", "lite.subscription.get"],
    "go.providerRouting.set": "lite.subscription.get", "go.allowTraining.set": "lite.subscription.get",
    "go.referral.reward.apply": ["go.referral.get", "go.referral.usagePreview", "lite.subscription.get"],
  }
  const required = (name: string, error: string, trim = false) => {
    const value = trim ? field(name)?.trim() : field(name)
    if (!value) return { error }
    input[name] = value
  }
  if (form) {
    if (["key.create", "workspace.update"].includes(operation)) {
      const error = required("name", operation === "key.create" ? "error.nameRequired" : "error.workspaceNameRequired", true)
      if (error) return error
      if (operation === "workspace.update" && String(input.name).length > 255) return { error: "error.nameTooLong" }
    }
    if (["key.remove", "member.update", "member.remove"].includes(operation)) {
      const error = required("id", "error.idRequired")
      if (error) return error
      call.resource = String(input.id)
      delete input.id
    }
    if (!workspace && !["createWorkspace", "emailSignup"].includes(operation)) return { error: "error.workspaceRequired" }
    if (operation === "emailSignup") input.email = field("email")
    if (operation === "createWorkspace") {
      const name = field("workspaceName")?.trim()
      if (!name) return
      call.operation = "workspace.create"
      input.name = name
    }
    if (["member.create", "member.update"].includes(operation)) {
      if (operation === "member.create") { const error = required("email", "error.emailRequired", true); if (error) return error }
      const error = required("role", "error.roleRequired"); if (error) return error
      const limit = field("limit")
      input.monthlyLimit = limit?.trim() ? parseInt(limit) : null
      if (typeof input.monthlyLimit === "number" && input.monthlyLimit < 0) return { error: "error.monthlyLimitInvalid" }
    }
    if (["provider.save", "provider.remove"].includes(operation)) {
      const error = required("provider", "error.providerRequired"); if (error) return error
      call.resource = String(input.provider)
      if (operation === "provider.save") { const error = required("credentials", "error.apiKeyRequired"); if (error) return error }
    }
    if (operation === "model.toggle") {
      const error = required("model", "error.modelRequired"); if (error) return error
      input.enabled = field("enabled") === "true"
    }
    if (operation === "billing.setMonthlyLimit") {
      if (!field("limit")) return { error: "error.limitRequired" }
      input.limit = parseInt(field("limit")!)
      if (Number(input.limit) < 0) return { error: "error.monthlyLimitInvalid" }
    }
    if (operation === "billing.setReload") {
      input.reload = field("reload") === "true"
      for (const name of ["reloadAmount", "reloadTrigger"]) if (field(name)?.trim()) input[name] = parseInt(field(name)!)
      if (input.reload && (typeof input.reloadAmount !== "number" || input.reloadAmount < 10)) return { error: "error.reloadAmountMin:10" }
      if (input.reload && (typeof input.reloadTrigger !== "number" || input.reloadTrigger < 5)) return { error: "error.reloadTriggerMin:5" }
    }
    if (operation === "billing.redeemCoupon") {
      input.code = field("code")?.trim().toUpperCase()
      if (!input.code) return { error: "Coupon code is required." }
    }
    if (["setUseBalance", "setLiteUseBalance"].includes(operation)) input.useBalance = field("useBalance") === "true"
    if (operation === "go.providerRouting.set") input.region = field("useChinaProviders") === "true" ? ["us", "eu", "sg"] : ["us", "eu", "sg", "cn"]
    if (operation === "go.allowTraining.set") input.allow_training = field("allowTraining") === "true"
  }
  if (operation === "go.referral.usagePreview" && !args[1]) return null
  if (operation === "getTaskDetail") call.resource = String(args[1])
  if (operation === "usage.list") call.page = Number(args[1] ?? 0)
  if (operation === "checkoutUrl") Object.assign(input, { amount: args[1], successUrl: args[2], cancelUrl: args[3] })
  if (["sessionUrl", "liteSessionUrl"].includes(operation)) input.returnUrl = args[1]
  if (operation === "receipt.download") input.paymentID = args[1]
  if (operation === "liteCheckoutUrl") Object.assign(input, { successUrl: args[1], cancelUrl: args[2], method: args[3] })
  if (["go.referral.usagePreview", "go.referral.reward.apply"].includes(operation)) input.referralID = args[1]
  if (operation === "lite.subscription.usage") input.window = args[1]
  if (operation === "getCosts") Object.assign(input, { year: args[1], month: args[2], tzOffset: args[3] })
  if (operation === "checkLoggedIn.get") call.operation = "lastWorkspace"
  call.input = Object.keys(input).length ? input : ["enroll", "go.referral.capture"].includes(operation) ? {} : undefined
  const headers = new Headers({ "content-type": "application/json", origin: requestOrigin })
  if (event.request.headers.has("cookie")) headers.set("cookie", event.request.headers.get("cookie")!)
  if (event.request.headers.has("referer")) headers.set("referer", event.request.headers.get("referer")!)
  const response = await fetch(new URL("/_console/browser", backend()), { method: "POST", headers, body: JSON.stringify(call), redirect: "manual", signal: AbortSignal.timeout(30_000) })
  for (const cookie of response.headers.getSetCookie()) event.response.headers.append("set-cookie", cookie)
  if (response.headers.has("cache-control")) event.response.headers.set("cache-control", response.headers.get("cache-control")!)
  const location = response.headers.get("location")
  if (location) {
    if (operation === "checkLoggedIn.get" || operation === "auth.logout") return
    throw redirect(location, { status: response.status })
  }
  const result = response.status === 204 ? undefined : await response.json()
  if (operation === "changelog") return result.releases ?? []
  if (operation === "github" && (!response.ok || result === null)) return undefined
  if (!response.ok) {
    if (operation === "checkLoggedIn.get") return
    if (revalidate[operation] || operation === "checkoutUrl") return { error: result.error ?? `Rust console HTTP ${response.status}` }
    throw new Error(result.error ?? `Rust console HTTP ${response.status}`)
  }
  if (operation === "session.get") return { isAdmin: result.isAdmin, isBeta: result.isBeta }
  if (operation === "member.list") {
    const actor = await invoke("member.actor", [workspace])
    return { members: result, actorID: actor.actorID, actorRole: actor.actorRole }
  }
  if (operation === "createWorkspace") return redirect(`/workspace/${result.id}`)
  if (operation === "checkLoggedIn.get") return result ?? undefined
  if (operation === "billing.redeemCoupon") return json(result?.error ? result : { data: true }, { revalidate: revalidate[operation] })
  if (operation === "checkoutUrl" || ["sessionUrl", "liteSessionUrl", "liteCheckoutUrl", "key.create", "member.create", "member.update", "member.remove"].includes(operation))
    return json({ data: result }, { revalidate: revalidate[operation] })
  if (revalidate[operation]) return json(result ?? {}, { revalidate: revalidate[operation] })
  return result
}