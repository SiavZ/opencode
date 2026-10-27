// Purpose: build unchanged support UI with one native SSR query delegation.
// Scope: isolated local artifacts only, no baseline writes or public deployment.
// Invariants: no runtime secret needed at build; no direct DB fallback.
// Contract tests: plugin.test.ts and acceptance.ts.
import { createRequire } from "node:module"
import { resolve } from "node:path"
import { fileURLToPath,pathToFileURL } from "node:url"
import { mkdir,writeFile } from "node:fs/promises"
import { supportRoot,supportBridge } from "./plugin"
export const outputRoot = fileURLToPath(new URL("../../target/console-support/",import.meta.url))
export async function build() {
  const home=resolve(outputRoot,"home"); await mkdir(home,{recursive:true})
  process.env.HOME=home
  for(const name of ["CONFIG","DATA","CACHE"]) process.env[`XDG_${name}_HOME`]=resolve(home,name.toLowerCase())
  const require=createRequire(resolve(supportRoot,"package.json"))
  const framework=createRequire(require.resolve("@solidjs/start/config"))
  const {createBuilder}=await import(pathToFileURL(framework.resolve("vite")).href)
  const {solidStart}=await import(pathToFileURL(require.resolve("@solidjs/start/config")).href)
  const {nitro}=await import(pathToFileURL(require.resolve("nitro/vite")).href)
  process.chdir(supportRoot)
  const builder=await createBuilder({configFile:false,root:supportRoot,cacheDir:resolve(outputRoot,"cache"),plugins:[supportBridge(),{name:"operator-support-topology",enforce:"post",configEnvironment(name,config){
    if(name!=="ssr")return
    config.resolve??={}; config.resolve.noExternal=true
    config.build??={}; config.build.outDir=resolve(outputRoot,"nitro/vite/services",name)
    config.build.rollupOptions??={};config.build.rollupOptions.input=fileURLToPath(new URL("./entry-server.ts",import.meta.url))
  }},solidStart(),nitro({preset:"node-server",compatibilityDate:"2024-09-19",buildDir:resolve(outputRoot,"nitro"),output:{dir:resolve(outputRoot,"output"),serverDir:resolve(outputRoot,"output/server"),publicDir:resolve(outputRoot,"output/public")}})],resolve:{alias:{"~":resolve(supportRoot,"src"),"solid-js":require.resolve("solid-js").replace(/dist\/[^/]+$/,""),"@solidjs/router":require.resolve("@solidjs/router").replace(/dist\/[^/]+$/,"")}},build:{outDir:resolve(outputRoot,"client"),emptyOutDir:false,minify:false}})
  await builder.buildApp()
  await writeFile(resolve(outputRoot,"topology.json"),JSON.stringify({mode:"local-operator-only",deployable:false,preset:"node-server",entry:resolve(outputRoot,"output/server/index.mjs")},null,2))
  console.log("SUPPORT_SSR_BUILT")
}
if(import.meta.main)await build()
