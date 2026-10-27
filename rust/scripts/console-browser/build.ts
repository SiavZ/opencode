// Purpose: build unchanged SolidStart UI with native Rust actions and Node SSR output.
// Scope: local production artifacts under rust/target, never cloud deployment.
// Uses: installed pinned SolidStart Vite, Nitro node-server, plugin.ts.
// Invariants: no packages/node_modules writes; API route bodies never run baseline cloud adapters.
// Contract tests: production.test.ts and local production acceptance.
import { createRequire } from "node:module"
import { resolve, relative } from "node:path"
import { pathToFileURL, fileURLToPath } from "node:url"
import { mkdir, writeFile } from "node:fs/promises"
import { appRoot, rustConsole } from "./plugin"
import { generateBuildArtifacts } from "./build-artifacts"

export const outputRoot = resolve(fileURLToPath(new URL("../../target/console-browser-production/", import.meta.url)))

export async function buildProduction() {
  await mkdir(outputRoot, { recursive: true })
  const isolated = resolve(outputRoot, "home")
  await mkdir(isolated, { recursive: true })
  process.env.HOME = isolated
  process.env.XDG_CONFIG_HOME = resolve(isolated, "config")
  process.env.XDG_DATA_HOME = resolve(isolated, "data")
  process.env.XDG_CACHE_HOME = resolve(isolated, "cache")
  const generatedPublic = resolve(outputRoot, "generated-public")
  await generateBuildArtifacts(generatedPublic, resolve(outputRoot, "artifact-home"))
  const require = createRequire(resolve(appRoot, "package.json"))
  const framework = createRequire(require.resolve("@solidjs/start/config"))
  const { createBuilder } = await import(pathToFileURL(framework.resolve("vite")).href)
  const { solidStart } = await import(pathToFileURL(require.resolve("@solidjs/start/config")).href)
  const { nitro } = await import(pathToFileURL(require.resolve("nitro/vite")).href)
  process.chdir(appRoot)
  const entry = fileURLToPath(new URL("./production-entry.ts", import.meta.url))
  const nativeBuild = {
    name: "rust-console-production-topology",
    enforce: "post" as const,
    configEnvironment(name, config) {
      if (name !== "ssr") return
      config.resolve ??= {}
      config.resolve.noExternal = true
      config.build ??= {}
      config.build.outDir = resolve(outputRoot, "nitro/vite/services", name)
      config.build.rollupOptions ??= {}
      config.build.rollupOptions.input = entry
    },
    transform(code, id) {
      const path = id.split("?")[0]
      const route = relative(resolve(appRoot, "src/routes"), path)
      if (route.startsWith("..") || /\.tsx$/.test(path) || !/^export (?:async )?function (?:GET|POST|PUT|PATCH|DELETE|OPTIONS|HEAD)|export const (?:GET|POST|PUT|PATCH|DELETE|OPTIONS|HEAD)/m.test(code)) return
      return { code: 'export const GET = () => new Response("Native route requires front proxy", {status:502}); export const POST=GET; export const PUT=GET; export const PATCH=GET; export const DELETE=GET; export const OPTIONS=GET; export const HEAD=GET;', map: null }
    },
  }
  const builder = await createBuilder({
    configFile: false,
    root: appRoot,
    cacheDir: resolve(outputRoot, "vite-cache"),
    plugins: [rustConsole(), nativeBuild, solidStart({ middleware: "./src/middleware.ts" }), nitro({
      preset: "node-server", compatibilityDate: "2024-09-19",
      publicAssets: [{ dir: generatedPublic, baseURL: "/" }],
      buildDir: resolve(outputRoot, "nitro"),
      output: { dir: resolve(outputRoot, "output"), serverDir: resolve(outputRoot, "output/server"), publicDir: resolve(outputRoot, "output/public") },
    })],
    resolve: { alias: {
      "solid-js": require.resolve("solid-js").replace(/dist\/[^/]+$/, ""),
      "@solidjs/router": require.resolve("@solidjs/router").replace(/dist\/[^/]+$/, ""),
    } },
    build: { outDir: resolve(outputRoot, "intermediate/client"), emptyOutDir: false, minify: false },
  })
  await builder.buildApp()
  await writeFile(resolve(outputRoot,"topology.json"),JSON.stringify({
    preset:"node-server", plugin:"rust-console-server-actions", entry:resolve(outputRoot,"output/server/index.mjs"),
    command:"node rust/scripts/console-browser/production-start.mjs", backend:"shared-loopback-network", deployable:false,
  },null,2))
  console.log(`RUST_CONSOLE_PRODUCTION_BUILT ${resolve(outputRoot,"output/server/index.mjs")}`)
}

if (import.meta.main) await buildProduction()
