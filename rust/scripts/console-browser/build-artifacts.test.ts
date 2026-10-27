// Purpose: prove native public artifacts match unchanged authoritative baseline producers exactly.
// Scope: disposable fixture directories under rust/target, no production or baseline writes.
// Uses: build-artifacts.ts, baseline sitemap script copied unchanged, original schema generator.
// Invariants: exact byte parity, no copied schema definitions, source drift fails closed.
// Contract tests: this file.
import { expect,test } from "bun:test"
import { mkdir,mkdtemp,readFile,symlink,writeFile } from "node:fs/promises"
import { resolve,dirname } from "node:path"
import { generateBuildArtifacts,isolatedSitemap,repository,runGenerator,schemaSource,sitemapSource } from "./build-artifacts"

test("generated assets enter Nitro public manifest before compilation",async()=>{
  const source=await Bun.file(new URL("./build.ts",import.meta.url)).text()
  expect(source).toContain('publicAssets: [{ dir: generatedPublic, baseURL: "/" }]')
  expect(source.indexOf("await generateBuildArtifacts(")).toBeLessThan(source.indexOf("await createBuilder("))
})

test("sitemap isolation fails closed on source drift",async()=>{
  const source=await readFile(sitemapSource,"utf8")
  expect(()=>isolatedSitemap(source.replace("void main()","main()"),"/fixture/public")).toThrow("source drift")
  expect(()=>isolatedSitemap(source+"\nvoid main()","/fixture/public")).toThrow("source drift")
})

test("all three native assets equal unchanged baseline generators byte for byte",async()=>{
  const target=resolve(repository,"rust/target/console-artifact-tests")
  await mkdir(target,{recursive:true})
  const fixture=await mkdtemp(resolve(target,"parity-"))
  const publicDir=resolve(fixture,"native/public")
  const baselineApp=resolve(fixture,"baseline/packages/console/app")
  const baselineScript=resolve(baselineApp,"script/generate-sitemap.ts")
  const baselinePublic=resolve(baselineApp,"public")
  await mkdir(dirname(baselineScript),{recursive:true})
  await mkdir(baselinePublic,{recursive:true})
  await symlink(resolve(repository,"packages/console/app/src"),resolve(baselineApp,"src"),"dir")
  await symlink(resolve(repository,"packages/web"),resolve(fixture,"baseline/packages/web"),"dir")
  const source=await readFile(sitemapSource,"utf8")
  await writeFile(baselineScript,source)
  await runGenerator([baselineScript],baselineApp,resolve(fixture,"baseline-home"))
  await runGenerator([schemaSource,resolve(baselinePublic,"config.json"),resolve(baselinePublic,"tui.json")],resolve(repository,"packages/opencode"),resolve(fixture,"schema-home"))
  const generated=await generateBuildArtifacts(publicDir,resolve(fixture,"native-home"))
  expect(generated.map(file=>file.name)).toEqual(["sitemap.xml","config.json","tui.json"])
  for(const name of ["sitemap.xml","config.json","tui.json"]){
    expect(await Bun.file(resolve(publicDir,name)).text()).toBe(await Bun.file(resolve(baselinePublic,name)).text())
  }
  const config=await Bun.file(resolve(publicDir,"config.json")).json()
  const tui=await Bun.file(resolve(publicDir,"tui.json")).json()
  expect(config.$schema).toBe("https://json-schema.org/draft/2020-12/schema")
  expect(config.allowComments).toBe(true)
  expect(tui.allowTrailingCommas).toBe(true)
  expect(await readFile(sitemapSource,"utf8")).toBe(source)
},120000)
