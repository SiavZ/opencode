// Purpose: opencode plugin compatibility bridge. Runs user JS/TS plugins inside bun or node and
// speaks the Rust-defined JSON-RPC ABI over stdio. GENERATED: the constants, dispatch table and host
// stubs between the markers are produced by integration-api/src/plugin_abi/bridge.rs from METHODS.
// Do not edit generated sections by hand.
import { createRequire } from "node:module"
import { createInterface } from "node:readline"

// ---- generated ----
__GENERATED__
// ---- end generated ----

// Frames are prefixed with an ASCII record separator so stray plugin writes to stdout never parse
// as protocol messages. console.log and friends are redirected to stderr.
const FRAME = "\u001e"
const rawWrite = process.stdout.write.bind(process.stdout)
for (const key of ["log", "info", "debug"]) console[key] = (...args) => console.error(...args)
const send = (msg) => rawWrite(FRAME + JSON.stringify(msg) + "\n")

class RpcError extends Error {
  constructor(code, message, data) {
    super(message)
    this.code = code
    this.data = data
  }
}

const pending = new Map()
let nextId = 1
function request(method, params) {
  const id = nextId++
  send({ jsonrpc: "2.0", id, method, params })
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }))
}
function notify(method, params) {
  send({ jsonrpc: "2.0", method, params })
}

function errorMessage(error) {
  if (error instanceof Error) return error.message
  if (typeof error === "string") return error
  try {
    return JSON.stringify(error) ?? String(error)
  } catch {
    return String(error)
  }
}
const isRecord = (v) => !!v && typeof v === "object" && !Array.isArray(v)
const typeOf = (v) => (v === null ? "null" : Array.isArray(v) ? "array" : typeof v)
const clone = (v) => {
  try {
    const text = JSON.stringify(v)
    return text === undefined ? null : JSON.parse(text)
  } catch {
    return null
  }
}

const state = { zodFallback: undefined, modules: new Map(), plugins: new Map(), calls: new Map(), next: 1 }

async function onInitialize(params) {
  if (params.abiVersion !== ABI_VERSION)
    throw new RpcError(ErrorCode.AbiMismatch, `bridge ABI ${ABI_VERSION} does not match host ABI ${params.abiVersion}`)
  state.zodFallback = params.zodFallback
  const bun = typeof Bun !== "undefined"
  return { abiVersion: ABI_VERSION, runtime: bun ? "bun" : "node", runtimeVersion: bun ? Bun.version : process.versions.node }
}

async function onModuleImport(params) {
  const mod = await import(params.entry)
  const handle = state.next++
  state.modules.set(handle, { mod, entry: params.entry })
  const ids = new Map()
  const exports = Object.entries(mod).map(([name, value]) => {
    if (!ids.has(value)) ids.set(value, ids.size + 1)
    const shape = { name, type: typeOf(value), identity: ids.get(value) }
    if (isRecord(value) || typeof value === "function") {
      if ("id" in value) shape.idType = typeOf(value.id)
      if (typeof value.id === "string") shape.id = value.id
      if ("server" in value) shape.server = typeOf(value.server)
      if ("tui" in value) shape.tui = typeOf(value.tui)
    }
    return shape
  })
  return { module: handle, exports }
}

function createClient(path = []) {
  const call = (...args) => host.clientCall({ path, args: args.map(clone) })
  return new Proxy(call, {
    get(_target, key) {
      if (key === "then" || typeof key === "symbol") return undefined
      return createClient([...path, key])
    },
  })
}

async function onPluginInit(params) {
  const loaded = state.modules.get(params.module)
  if (!loaded) throw new RpcError(ErrorCode.NotFound, `module ${params.module} not found`)
  const value = loaded.mod[params.export]
  const fn = params.member === "server" ? value?.server : value
  if (typeof fn !== "function") throw new RpcError(ErrorCode.InvalidParams, `export ${params.export} is not a plugin function`)
  const input = {
    client: createClient(),
    project: params.input.project,
    worktree: params.input.worktree,
    directory: params.input.directory,
    experimental_workspace: {
      register(type, adapter) {
        host.workspaceRegister({ type, name: String(adapter?.name ?? ""), description: String(adapter?.description ?? "") })
      },
    },
    get serverUrl() {
      return new URL(params.input.serverUrl)
    },
    $: typeof Bun === "undefined" ? undefined : Bun.$,
  }
  const hooks = (await fn(input, params.options)) ?? {}
  const handle = state.next++
  const tools = await Promise.all(Object.entries(hooks.tool ?? {}).map(([id, def]) => toolShape(id, def, loaded.entry)))
  state.plugins.set(handle, { hooks, entry: loaded.entry })
  return {
    handle,
    hooks: {
      triggers: Object.keys(hooks).filter((key) => TRIGGERS.includes(key) && typeof hooks[key] === "function"),
      event: typeof hooks.event === "function",
      config: typeof hooks.config === "function",
      dispose: typeof hooks.dispose === "function",
      tools,
      ...(typeof hooks.auth?.provider === "string" ? { authProvider: hooks.auth.provider } : {}),
      ...(typeof hooks.provider?.id === "string" ? { provider: hooks.provider.id } : {}),
    },
  }
}

const isZodType = (v) => typeof v === "object" && v !== null && "_zod" in v

async function loadZod(entry) {
  const candidates = []
  try {
    candidates.push(createRequire(new URL(entry)).resolve("zod"))
  } catch {}
  if (state.zodFallback) candidates.push(state.zodFallback)
  for (const file of candidates) {
    try {
      const mod = await import(file.startsWith("file:") ? file : "file://" + file)
      const z = mod.z ?? mod.default ?? mod
      if (typeof z.toJSONSchema === "function" && typeof z.object === "function") return z
    } catch {}
  }
  throw new RpcError(ErrorCode.Unsupported, "zod could not be resolved from the plugin or the host fallback")
}

// Mirrors zodMetadataRegistry in packages/opencode/src/tool/registry.ts: carry descriptions and
// .meta() across zod instances.
function metadataRegistry(z, schema) {
  const registry = z.registry()
  const seen = new WeakSet()
  const collect = (value) => {
    if (typeof value !== "object" || value === null || seen.has(value)) return
    seen.add(value)
    if (isZodType(value)) {
      const metadata = typeof value.meta === "function" ? value.meta() : undefined
      const description = typeof value.description === "string" ? value.description : undefined
      const merged = { ...(metadata && typeof metadata === "object" ? metadata : {}), ...(description ? { description } : {}) }
      if (Object.keys(merged).length) registry.add(value, merged)
      collect(value._zod.def)
      return
    }
    for (const item of Object.values(value)) collect(item)
  }
  collect(schema)
  return registry
}

async function toolShape(id, def, entry) {
  const args = def?.args ?? {}
  const entries = Object.entries(args)
  const description = String(def?.description ?? "")
  if (!entries.every((e) => isZodType(e[1]))) return { id, description, args: { kind: "legacy", entries: Object.fromEntries(entries.map(([k, v]) => [k, clone(v)])) } }
  try {
    const z = await loadZod(entry)
    const schema = z.object(args)
    return { id, description, args: { kind: "zod", schema: z.toJSONSchema(schema, { io: "input", metadata: metadataRegistry(z, schema) }) } }
  } catch (error) {
    return { id, description, args: { kind: "error", message: errorMessage(error) } }
  }
}

function plugin(handle) {
  const hit = state.plugins.get(handle)
  if (!hit) throw new RpcError(ErrorCode.NotFound, `plugin ${handle} not found`)
  return hit
}

async function onHookTrigger(params) {
  const fn = plugin(params.handle).hooks[params.name]
  if (!fn) return { output: params.output }
  if (typeof fn !== "function") throw new RpcError(ErrorCode.PluginThrew, `hook ${params.name} is not a function`)
  const output = params.output
  await fn(params.input, output)
  return { output: clone(output) }
}

async function onHookEvent(params) {
  const hooks = plugin(params.handle).hooks
  try {
    await hooks.event?.({ event: params.event })
  } catch (error) {
    host.log({ level: "error", message: `plugin event hook failed: ${errorMessage(error)}` })
  }
}

async function onHookConfig(params) {
  await plugin(params.handle).hooks.config?.(params.config)
  return null
}

async function onHookDispose(params) {
  await plugin(params.handle).hooks.dispose?.()
  return null
}

async function onToolExecute(params) {
  const hit = plugin(params.handle)
  const def = hit.hooks.tool?.[params.tool]
  if (!def || typeof def.execute !== "function") throw new RpcError(ErrorCode.NotFound, `tool ${params.tool} not found`)
  const args = def.args ?? {}
  if (Object.entries(args).every((e) => isZodType(e[1]))) {
    const z = await loadZod(hit.entry)
    const parsed = z.object(args).safeParse(params.args)
    if (!parsed.success) throw new RpcError(ErrorCode.Validation, parsed.error.message, clone(parsed.error.issues))
  }
  const controller = new AbortController()
  state.calls.set(params.callId, controller)
  const callId = params.callId
  const ctx = {
    ...params.context,
    abort: controller.signal,
    metadata(input) {
      host.ctxMetadata({ callId, input: clone(input) })
    },
    ask(input) {
      return host.ctxAsk({ callId, input: clone(input) }).then(() => undefined)
    },
  }
  try {
    return clone(await def.execute(params.args, ctx))
  } finally {
    state.calls.delete(callId)
  }
}

async function onToolAbort(params) {
  state.calls.get(params.callId)?.abort()
}

async function onAuthMethods(params) {
  const auth = plugin(params.handle).hooks.auth
  if (!auth) return null
  return {
    provider: auth.provider,
    methods: auth.methods.map((method) => ({
      type: method.type,
      label: method.label,
      ...(method.prompts ? { prompts: method.prompts.map((prompt) => ({
        type: prompt.type, key: prompt.key, message: prompt.message,
        ...(prompt.type === "select" ? { options: clone(prompt.options) } : prompt.placeholder ? { placeholder: prompt.placeholder } : {}),
        ...(prompt.when ? { when: clone(prompt.when) } : {}),
      })) } : {}),
    })),
  }
}

async function onAuthAuthorize(params) {
  const hit = plugin(params.handle)
  const method = hit.hooks.auth?.methods?.[params.method]
  if (!method) throw new RpcError(ErrorCode.InvalidParams, "auth method not found")
  if (method.type !== "oauth") return null
  for (const prompt of method.prompts ?? []) {
    if (prompt.type !== "text" || !prompt.validate || params.inputs?.[prompt.key] === undefined) continue
    const message = prompt.validate(params.inputs[prompt.key])
    if (message) throw new RpcError(ErrorCode.Validation, message, { name: "ProviderAuthValidationFailed", field: prompt.key, message })
  }
  const result = await method.authorize(params.inputs ?? undefined)
  if (params.attemptID) {
    hit.authAttempts ??= new Map()
    hit.authAttempts.set(params.attemptID, result)
  } else hit.pendingAuth = result
  return { url: result.url, method: result.method, instructions: result.instructions }
}

async function onAuthCallback(params) {
  const hit = plugin(params.handle)
  const match = params.attemptID ? hit.authAttempts?.get(params.attemptID) : hit.pendingAuth
  if (!match) throw new RpcError(ErrorCode.NotFound, "OAuth authorization missing", { name: "ProviderAuthOauthMissing", providerID: plugin(params.handle).hooks.auth?.provider })
  if (match.method === "code" && !params.code)
    throw new RpcError(ErrorCode.InvalidParams, "OAuth code missing", { name: "ProviderAuthOauthCodeMissing", providerID: plugin(params.handle).hooks.auth?.provider })
  const result = await (match.method === "code" ? match.callback(params.code) : match.callback())
  if (!result || result.type !== "success")
    throw new RpcError(ErrorCode.PluginThrew, "OAuth callback failed", { name: "ProviderAuthOauthCallbackFailed" })
  return clone(result)
}

async function onAuthCancel(params) {
  plugin(params.handle).authAttempts?.delete(params.attemptID)
  return null
}

async function onShutdown() {
  setTimeout(() => process.exit(0), 0)
  return null
}

function reply(id, run) {
  run.then(
    (result) => id !== undefined && send({ jsonrpc: "2.0", id, result: result ?? null }),
    (error) => {
      if (id === undefined) return host.log({ level: "error", message: errorMessage(error) })
      const code = error instanceof RpcError ? error.code : ErrorCode.PluginThrew
      const data = error instanceof RpcError ? error.data : error instanceof Error && error.stack ? { stack: error.stack } : undefined
      send({ jsonrpc: "2.0", id, error: { code, message: errorMessage(error), ...(data === undefined ? {} : { data }) } })
    },
  )
}

createInterface({ input: process.stdin }).on("line", (line) => {
  if (!line.trim()) return
  let msg
  try {
    msg = JSON.parse(line)
  } catch (error) {
    return send({ jsonrpc: "2.0", id: null, error: { code: ErrorCode.ParseError, message: errorMessage(error) } })
  }
  if (msg.method === undefined) {
    const waiter = pending.get(msg.id)
    if (!waiter) return
    pending.delete(msg.id)
    if (msg.error) return waiter.reject(new Error(msg.error.message))
    return waiter.resolve(msg.result)
  }
  const handler = HANDLERS[msg.method]
  if (!handler) {
    if (msg.id !== undefined) send({ jsonrpc: "2.0", id: msg.id, error: { code: ErrorCode.MethodNotFound, message: `unknown method ${msg.method}` } })
    return
  }
  reply(msg.id, Promise.resolve().then(() => handler(msg.params ?? {})))
}).on("close", () => process.exit(0))
