// Purpose: exercise unchanged frontend SSR and real browser actions against isolated Rust fixtures.
// Scope: executable local Chromium acceptance, never production credentials or endpoints.
// Uses: installed Playwright, start.ts; Rust fixture supplies loopback address and sealed cookie.
// Invariants: no protocol reimplementation; browser-generated SolidStart POST must persist in Rust.
// Contract tests: console::browser::tests::fixture_browser_ssr_and_action_round_trip.
import { createRequire } from "node:module"
import { resolve } from "node:path"
import { appRoot } from "./plugin"

const require = createRequire(resolve(appRoot, "package.json"))
const { chromium } = require(require.resolve("playwright", { paths: [resolve(appRoot, "../../app"), resolve(appRoot, "../../../node_modules/.bun/@playwright+test@1.59.1/node_modules")] }))
const origin = process.env.OPENCODE_CONSOLE_PUBLIC_ORIGIN!
const workspace = process.env.OPENCODE_BROWSER_TEST_WORKSPACE!
const cookie = process.env.OPENCODE_BROWSER_TEST_COOKIE!
if (!origin || !workspace || !cookie || new URL(origin).hostname !== "127.0.0.1") throw new Error("Local fixture environment required")
const production = !!process.env.OPENCODE_BROWSER_PRODUCTION_ENTRY
const vite = Bun.spawn(production ? ["node", resolve(import.meta.dir, "production-start.mjs")] : ["bun", resolve(import.meta.dir, "start.ts")], { env: process.env, stdout: "pipe", stderr: "pipe" })
const output = new Response(vite.stdout).text()
const errors = new Response(vite.stderr).text()
const browser = await chromium.launch({ headless: true }).catch(async (error) => {
  vite.kill("SIGTERM")
  await vite.exited
  throw error
})
try {
  const context = await browser.newContext()
  await context.addCookies([{ name: "auth", value: cookie.slice("auth=".length).split(";")[0], url: origin, httpOnly: true }])
  const page = await context.newPage()
  const consoleErrors: string[] = []
  page.on("pageerror", (error) => consoleErrors.push(error.message))
  for (let attempt = 0; attempt < 150; attempt++) {
    const response = await fetch(production ? `${origin}/workspace/${workspace}/settings` : `${origin}/@vite/client`, { headers: { cookie: cookie.split(";")[0] } }).catch(() => undefined)
    if (response?.ok) break
    if (attempt === 149) throw new Error("Local Vite failed to start")
    await Bun.sleep(100)
  }
  const document = await fetch(`${origin}/workspace/${workspace}/settings`, { headers: { cookie: cookie.split(";")[0] }, redirect: "manual" })
  const html = await document.text()
  if (document.status !== 200 || !html.includes("Browser fixture workspace")) throw new Error(`SSR failed ${document.status}: ${html.slice(-6000)}`)
  console.log("JCODE_CHECKPOINT " + JSON.stringify({ message: "Unchanged frontend SSR read authenticated Rust workspace" }))
  await page.goto(`${origin}/workspace/${workspace}/settings`, { waitUntil: "networkidle" })
  // First dev navigation can trigger Vite's dependency optimizer reload. Interact only
  // after the hydrated page has completed that reload, never against SSR-only controls.
  await page.reload({ waitUntil: "networkidle" })
  await page.locator('[data-slot="value-with-action"] button').click()
  await page.locator('input[name="name"]').waitFor({ timeout: 5000 }).catch(async (error) => {
    console.error("HYDRATION_DIAGNOSTIC", JSON.stringify({ errors: consoleErrors, html: await page.locator('[data-slot=setting]').innerHTML(), scripts: await page.locator('script[src]').evaluateAll((nodes) => nodes.map((node) => node.getAttribute('src'))) }))
    throw error
  })
  await page.locator('input[name="name"]').fill("Browser action persisted")
  const actionResponse = page.waitForResponse((response) => new URL(response.url()).pathname === "/_server" && response.request().method() === "POST")
  await page.locator('form[data-slot="create-form"] button[type="submit"]').click()
  const response = await actionResponse
  const wire = await response.text()
  if (response.status() !== 200 || !response.headers()["content-type"]?.includes("text/javascript") || !wire.startsWith(";0x")) throw new Error(`Unexpected SolidStart response ${response.status()}: ${wire}`)
  if (response.headers()["x-error"]) throw new Error(`Server action failed: ${wire}`)
  await page.locator('[data-slot="current-value"]').filter({ hasText: "Browser action persisted" }).waitFor()
  await page.reload()
  await page.locator('[data-slot="current-value"]').filter({ hasText: "Browser action persisted" }).waitFor()
  // Import actual Vite-generated client functions. This invokes the installed SolidStart
  // request encoder and response decoder, including Seroval JSON positional arguments.
  const rpc = production ? { mode: "compiled-ssr" } : await page.evaluate(async ({ workspace }) => {
    const source = await fetch('/src/routes/workspace/[id]/provider-section.tsx').then((r) => r.text())
    const refs = [...source.matchAll(/const (\w+) = createServerReference\("([^"]+)"\)/g)]
    const runtimePath = /from "([^"]*server-runtime\.js[^\"]*)"/.exec(source)?.[1]
    if (!runtimePath) throw new Error("Compiler client runtime import not found")
    const { createServerReference } = await import(runtimePath)
    const ref = (name: string) => {
      const match = refs.find((entry) => entry[1] === name)
      if (!match) throw new Error(`Compiler reference ${name} not found`)
      return createServerReference(match[2])
    }
    const form = new FormData()
    form.set("workspaceID", workspace)
    form.set("provider", "openai")
    form.set("credentials", "sk-local-browser-fixture")
    const saved = await ref("saveProvider_action")(form)
    if (!(saved instanceof Response) || saved.headers.get("x-revalidate") !== "provider.list") throw new Error("Provider revalidation missing")
    const rows = await ref("listProviders_query")(workspace)
    if (rows[0]?.credentials !== "sk-local-browser-fixture") throw new Error("Provider mutation not persisted")
    const getRows = await ref("listProviders_query").GET(workspace)
    if (getRows[0]?.credentials !== "sk-local-browser-fixture") throw new Error("GET Seroval arguments did not reach Rust")
    const invalid = new FormData()
    invalid.set("workspaceID", workspace)
    invalid.set("provider", "openai")
    const validation = await ref("saveProvider_action")(invalid)
    if (validation.error !== "error.apiKeyRequired") throw new Error("Baseline form validation was lost")
    const graphSource = await fetch('/src/routes/workspace/[id]/usage/graph-section.tsx').then((r) => r.text())
    const graphID = /getCosts\w* = createServerReference\("([^"]+)"\)/.exec(graphSource)?.[1]
    if (!graphID) throw new Error(`Compiler graph reference not found: ${graphSource.slice(0, 8000)}`)
    const graph = await createServerReference(graphID)(workspace, 2026, 9, "+02:00")
    if (!Array.isArray(graph.usage) || !Array.isArray(graph.keys)) throw new Error("Graph Seroval response shape invalid")
    const billingSource = await fetch('/src/routes/workspace/[id]/billing/monthly-limit-section.tsx').then((r) => r.text())
    const billingID = /const setMonthlyLimit_action = createServerReference\("([^"]+)"\)/.exec(billingSource)?.[1]
    if (!billingID) throw new Error("Compiler monthly-limit reference not found")
    const limit = new FormData()
    limit.set("workspaceID", workspace)
    limit.set("limit", "42")
    const updated = await createServerReference(billingID)(limit)
    if (!(updated instanceof Response) || updated.headers.get("x-revalidate") !== "billing.get") throw new Error("Billing revalidation missing")
    return { graphKeys: graph.keys.length, providers: rows.length }
  }, { workspace })
  const localized = await fetch(`${origin}/fr/workspace/${workspace}/settings?ref=ab-cd1234zz++`, { headers: { cookie: cookie.split(";")[0] }, redirect: "manual" })
  const localizedHtml = await localized.text()
  const setCookies = localized.headers.getSetCookie().join(";")
  if (localized.status !== 200 || !localizedHtml.includes('lang="fr"') || !setCookies.includes("oc_locale=fr") || !setCookies.includes("oc_referral=ABCD1234ZZ")) throw new Error("Locale/referral middleware did not preserve baseline behavior")
  console.log("JCODE_CHECKPOINT " + JSON.stringify({ message: production ? "Compiled SSR hydrated UI action and locale/referral middleware passed" : "Provider FormData mutation, graph Seroval query, billing mutation and locale/referral middleware passed", ...rpc }))
  if (consoleErrors.length) throw new Error(`Browser errors: ${consoleErrors.join("; ")}`)
  console.log("JCODE_CHECKPOINT " + JSON.stringify({ message: "Chromium submitted compiler-generated multipart SolidStart action; Rust DB mutation survived reload" }))
  await context.close()
} finally {
  vite.kill("SIGTERM")
  await Promise.all([browser.close(), vite.exited])
  console.log(await output)
  console.error(await errors)
}