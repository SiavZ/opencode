# Phase 03 providers: status and blocker table (node p03.providers)

Owner: p03.providers. Code: `rust/crates/provider-adapters/src/providers/**`.
Tests: `rust/crates/provider-adapters/tests/providers_{facades,gateways,cloud,registry,transform}.rs`
(all fixture-based over wiremock on localhost; no live provider calls).

## What is implemented

| Area | Baseline | Rust |
| --- | --- | --- |
| Facade config, `AuthOptions.bearer`, `Auth.optional(..).orElse(Auth.config(..))`, `mergeRouteDefaults`, `Endpoint.merge` | `packages/llm/src/route/{auth-options,client,endpoint}.ts` | `providers/config.rs` (injectable `Env`, read at request time) |
| OpenAI facade + `openai-options` (store:false, GPT-5 reasoning defaults, textVerbosity) | `packages/llm/src/providers/{openai,openai-options}.ts` | `providers/openai.rs` |
| OpenAI-compatible facade + 9 profiles | `openai-compatible.ts`, `openai-compatible-profile.ts` | `providers/compatible.rs` |
| Anthropic, Google, xAI facades | `anthropic.ts`, `google.ts`, `xai.ts` | `providers/native.rs` |
| Amazon Bedrock facade + cross-region prefixes + Mantle selection | `amazon-bedrock.ts`, `core/.../provider/amazon-bedrock.ts`, legacy `provider.ts:336-498` | `providers/bedrock.rs`, `providers/cloud.rs` |
| Azure + GitHub Copilot facades (`shouldUseResponsesApi`) | `azure.ts`, `github-copilot.ts` | `providers/azure.rs` |
| OpenRouter (`openrouter-chat` protocol) + Cloudflare AI Gateway / Workers AI | `openrouter.ts`, `cloudflare.ts` | `providers/gateways.rs`, `providers/wrap.rs` |
| Registry over all 34 `ProviderPlugins` ids, per-package route rules | `core/src/plugin/provider.ts:36-71`, `plugin/provider/*.ts` | `providers/inventory.rs`, `providers/registry.rs`, `providers/registry_rules.rs` |
| Catalog transforms + legacy special loaders (attribution headers, anthropic-beta, opencode `public` key, Azure resource, Vertex project/location/URL templates, Cloudflare account/gateway/token, Snowflake account/token, error messages) | `core/.../provider/*.ts`, legacy `provider.ts:209-1050` | `providers/loaders.rs` |
| Vertex Gemini (express key / ADC auth) and Vertex Anthropic (`streamRawPredict`, `anthropic_version: vertex-2023-10-16`, no `model`) | `@ai-sdk/google-vertex{,/anthropic}` | `providers/cloud.rs` |
| Snowflake Cortex `cortexFetch` (`max_tokens` -> `max_completion_tokens`, `"role":""` repair) | `core/.../snowflake-cortex.ts` | `providers/cloud.rs` |
| Pure transform rules: `temperature`, `topP`, `topK`, `maxOutputTokens`, `sdkKey`, `providerOptions` (gateway slugs, Azure dual keys, OpenAI reasoning gate, Anthropic block binding), `smallOptions`, `sanitizeSurrogates` | `opencode/src/provider/transform.ts` | `providers/transform.rs` |
| Error mapping `parseStreamError`, `parseAPICallError` | `opencode/src/provider/error.ts` | `providers/error.rs` |

`RouteFactory` port: the registry's input is `providers::CatalogRoute` (catalog `provider.api`,
`request.headers/body`, `model.api.id`, credential, env, optional pre-built auth) and the output is
an `opencode_llm_api::Model`. p03.catalog has not landed its `catalog/` API yet; the catalog/
composition layer should adapt its `RouteFactory` (session-runtime `runner::model::RouteFactory`
takes a `RouteSpec`) by building a `CatalogRoute` and calling `ProviderRegistry::resolve`.

## Blocker table

All rows return `LlmErrorReason::NoRoute { route: "<what> (not yet ported)" }` via
`crate::unsupported::not_yet_ported` (non-retryable), asserted in
`tests/providers_registry.rs::every_plugin_id_resolves_or_fails_typed` and `providers_cloud.rs`.

| item | missing behavior | why | owner | proof needed |
| --- | --- | --- | --- | --- |
| cohere (`@ai-sdk/cohere`) | Cohere v2 chat wire protocol | no Rust protocol family; not OpenAI-compatible | p03 parent (new protocol family) | cassette-backed cohere protocol tests |
| gateway (`@ai-sdk/gateway`, Vercel AI Gateway v3) | gateway v3 protocol and auth | proprietary protocol, no Rust family | p03 parent | recorded gateway cassette |
| gitlab (`gitlab-ai-provider`) | Duo agentic chat / workflow chat, workflow model discovery, `aiGatewayHeaders`, feature flags | external SDK runtime with its own websocket/workflow protocol | p03 parent / Phase 07 boundary | GitLab Duo fixture capture |
| sap-ai-core | dynamic SDK factory with service key, deployment id, resource group | npm package loaded at runtime | Phase 07 (third-party interpreter boundary) | approved loader design |
| dynamic-provider (any unknown package, `file://` factories) | `npm.add` + dynamic import of `create*` factory | arbitrary JS runtime | Phase 07 | approved loader design |
| native provider api (`api.type == "native"`) | non-AI-SDK native settings | no baseline route mapping in core runner either | p03.catalog | catalog contract |
| google-vertex / google-vertex-anthropic without supplied `auth` | Google ADC access token acquisition (`google-auth-library`) | credential acquisition belongs to p03.auth | p03.auth | ADC token source wired into `CatalogRoute.auth` |
| amazon-bedrock without bearer or env keys | AWS default credential chain (profile, SSO, web identity, container, process) | credential acquisition belongs to p03.auth | p03.auth | chain source wired into `CatalogRoute.auth` |
| amazon-bedrock/mantle without bearer | SigV4 signing for Mantle (OpenAI wire on `bedrock-mantle`) | SigV4 route exists only for Converse | p03.messages / p03.auth | mantle SigV4 cassette |
| groq / mistral / perplexity / alibaba / vercel / venice / togetherai / cerebras | routed over OpenAI-compatible Chat; SDK-specific extras (groq reasoning format, mistral tool-id rules and `safe_prompt`, perplexity citations/search metadata, venice parameters) not lowered | SDKs are Chat-shaped but each adds provider fields | p03.providers follow-up | per-SDK cassette diff against TS |
| cloudflare-ai-gateway | binding path (`env.AI.run`) and Unified-API multi-step `cf-aig-step` handling; we post to `/compat/chat/completions` instead of the REST unified envelope | ai-gateway-provider REST envelope not ported | p03.providers follow-up | TS capture of REST envelope |
| transform.ts message normalization | `normalizeMessages`, `applyCaching`, `unsupportedParts`, `mapProviderOptions`, `message()` | operates on AI SDK `ModelMessage`, consumer is session request assembly (Phase 04 legacy runtime) | p03.providers follow-up / Phase 04 | differential tests over `ModelMessage` fixtures |
| transform.ts `variants`, `reasoningVariants`, `options()`, `schema()` (gemini/moonshot/openai sanitize lives in `tool_schema.rs`) | per-model reasoning variant tables, per-provider option defaults (toolStreaming, usage include, thinking budgets, kimi adaptive) | ~900 lines of model-id tables; not reached by native routes yet | p03.providers follow-up | port with the 6000-line transform.test.ts cases |
| `aisdk.ts` Responses input `id` stripping when `store !== true` | applied in `prepareOptions` fetch wrapper for openai/azure/mantle | Responses protocol owner should decide (body builder already emits `item_reference`) | p03.openai | Responses differential test |
| header timeout (`OPENAI_HEADER_TIMEOUT_DEFAULT = 300000`) and `chunkTimeout` | per-provider timeouts | executor supports `header_timeout`; registry does not set it yet | p03.providers follow-up | executor timeout test via registry |
| User-Agent OS release | `os.release()` in Cloudflare/GitLab UAs | registry takes `os_release` from the composition root (empty by default) | composition root | integration check |
