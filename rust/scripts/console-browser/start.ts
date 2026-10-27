// Purpose: run unchanged console frontend with local Rust server-action delegation.
// Scope: loopback-only development integration, never a cloud deployment.
// Uses: installed Vite and SolidStart, plugin.ts, runtime.ts.
// Invariants: packages/ and node_modules remain read-only; caches live under rust/target.
// Contract tests: acceptance.test.ts.
import { createRequire } from "node:module"
import { resolve } from "node:path"
import { mkdir, mkdtemp } from "node:fs/promises"
import { pathToFileURL } from "node:url"
import { appRoot, rustConsole } from "./plugin"

const scratch = resolve(process.env.JCODE_SCRATCH_DIR ?? resolve(appRoot, "../../../rust/target/console-browser"))
await mkdir(scratch, { recursive: true })
const isolated = await mkdtemp(resolve(scratch, "vite-"))
process.env.HOME = isolated
process.env.XDG_CONFIG_HOME = resolve(isolated, "config")
process.env.XDG_DATA_HOME = resolve(isolated, "data")
process.env.XDG_CACHE_HOME = resolve(isolated, "cache")
const port = Number(process.env.OPENCODE_BROWSER_PORT ?? 3001)
process.env.OPENCODE_CONSOLE_PUBLIC_ORIGIN ??= `http://127.0.0.1:${port}`
if (process.env.OPENCODE_CONSOLE_PUBLIC_ORIGIN !== `http://127.0.0.1:${port}`) throw new Error("Local browser launcher requires its exact loopback public origin")
const backend = new URL(process.env.OPENCODE_BROWSER_RUST_URL ?? "http://127.0.0.1:8787")
if (backend.protocol !== "http:" || !["127.0.0.1", "localhost", "[::1]"].includes(backend.hostname) || backend.pathname !== "/" || backend.username || backend.password || backend.search || backend.hash) throw new Error("Rust backend must be a loopback HTTP origin")
const require = createRequire(resolve(appRoot, "package.json"))
process.chdir(appRoot)
const frameworkRequire = createRequire(require.resolve("@solidjs/start/config"))
const { createServer } = await import(pathToFileURL(frameworkRequire.resolve("vite")).href)
const { solidStart } = await import(pathToFileURL(require.resolve("@solidjs/start/config")).href)
const server = await createServer({
  configFile: false,
  root: appRoot,
  cacheDir: resolve(isolated, "vite-cache"),
  plugins: [rustConsole(), solidStart({ middleware: "./src/middleware.ts", solid: { hot: false }, devOverlay: false })],
  optimizeDeps: { include: ["source-map-js", "chart.js"] },
  resolve: { alias: {
    "solid-js": require.resolve("solid-js").replace(/dist\/[^/]+$/, ""),
    "@solidjs/router": require.resolve("@solidjs/router").replace(/dist\/[^/]+$/, ""),
    "source-map-js": frameworkRequire.resolve("source-map-js"),
  } },
  server: {
    host: "127.0.0.1", port, strictPort: true,
    fs: { allow: [resolve(appRoot, "../../.."), isolated] },
    proxy: Object.fromEntries(["/auth", "/api", "/stripe", "/bench/submission", "/changelog.json", "/github.json", "/openapi.json", "/oauth", "/zen/v1", "/zen/go/v1", "/workspaces", "/_console"].map((path) => [path, { target: backend.origin, changeOrigin: false }])),
  },
})
await server.listen()
console.log(`RUST_CONSOLE_BROWSER_READY ${process.env.OPENCODE_CONSOLE_PUBLIC_ORIGIN}`)
const stop = async () => { await server.close(); process.exit(0) }
process.once("SIGTERM", stop)
process.once("SIGINT", stop)