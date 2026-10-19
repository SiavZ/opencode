// Purpose: capture pure reasoning-variant results from the actual TypeScript baseline.
// Scope: extracts unchanged pure functions, erases types with Bun, no module side effects/network.
// Uses: packages/opencode/src/provider/transform.ts and util/iife.ts.
// Used by: provider-adapters/tests/providers_variants.rs.
// Invariants: expected behavior is executed baseline source, never a copied implementation.
const root = new URL("../../../", import.meta.url)
const source = await Bun.file(new URL("packages/opencode/src/provider/transform.ts", root)).text()
const section = (start: string, end: string) => source.slice(source.indexOf(start), source.indexOf(end))
const pure = [
  "const OUTPUT_TOKEN_MAX = 32000; const INCLUDE_ENCRYPTED_REASONING = ['reasoning.encrypted_content'];",
  "const iife = (fn: () => unknown) => fn();",
  section("function isKimiFamily", "function sdkKey"),
  section("const WIDELY_SUPPORTED_EFFORTS", "function anthropicBindsThinking"),
  section("const GEMINI_2_5_RE", "const GEMINI_MODELS_WITH_SAMPLING_DEFAULTS"),
  section("function isLegacyGemini", "export function options"),
  section("export function reasoningVariants", "export * as ProviderTransform"),
].join("\n")
const code = new Bun.Transpiler({ loader: "ts" }).transformSync(pure)
const baseline = new Function(code.replace(/^export /gm, "") + "\nreturn { variants, reasoningVariants };")()
const packages = ["@ai-sdk/openai", "@ai-sdk/openai-compatible", "@ai-sdk/anthropic", "@ai-sdk/amazon-bedrock", "@ai-sdk/google", "@ai-sdk/google-vertex", "@ai-sdk/github-copilot", "@openrouter/ai-sdk-provider", "@ai-sdk/gateway", "ai-gateway-provider", "@ai-sdk/azure", "@ai-sdk/groq", "@ai-sdk/mistral", "@jerome-benoit/sap-ai-provider-v2", "@ai-sdk/alibaba", "@ai-sdk/cohere"]
const ids = ["gpt-5", "gpt-5.1", "gpt-5.2", "gpt-5.3-codex", "gpt-5-pro", "gpt-5.2-pro", "gpt-5-chat", "gpt-5.2-chat", "o1-mini", "o3-deep-research", "claude-opus-4-5", "claude-sonnet-4-6", "claude-opus-4.7", "claude-sonnet-5", "anthropic.claude-opus-4-6-v1", "anthropic/claude-opus-4.7", "google/gemini-2.5-pro", "gemini-2.5-flash", "gemini-3-flash", "gemini-3.1-pro", "gemini-1.5-pro", "grok-3-mini", "minimax-m3", "glm-5.2", "deepseek-v4", "north-mini-code", "kimi-k2.6", "generic-reasoner", "mistral-small-2603"]
const fixtures: Record<string, unknown>[] = packages.flatMap((npm) => ids.flatMap((id) => ["2025-01-01", "2025-12-05"].map((release_date) => {
  const model = { id, providerID: "fixture", api: { id, npm, url: "http://127.0.0.1:1" }, capabilities: { reasoning: true }, limit: { output: 64000 }, release_date }
  return { model, expected: baseline.variants(model) }
})))
for (const npm of packages) for (const reasoning_options of [[], [{ type: "effort", values: [null, "low", "high", "max"] }], [{ type: "toggle" }], [{ type: "budget_tokens", min: 1024, max: 10000 }]]) {
  const model = { id: "claude-opus-4.7", providerID: "fixture", api: { id: "claude-opus-4.7", npm, url: "http://127.0.0.1:1" }, capabilities: { reasoning: true }, limit: { output: 64000 }, release_date: "2026-01-01" }
  const inventory = { reasoning_options }
  fixtures.push({ model, inventory, expected: baseline.reasoningVariants(inventory, model) ?? baseline.variants(model) })
}
await Bun.write(new URL("rust/contracts/phase-03/provider-variants.json", root), JSON.stringify(fixtures) + "\n")
console.log(`Captured ${fixtures.length} actual TS variant observations`)
