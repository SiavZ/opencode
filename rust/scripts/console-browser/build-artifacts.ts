// Purpose: generate native public assets using the authoritative baseline sitemap and schema producers.
// Scope: isolated build output only, no baseline files or production services are modified.
// Uses: current packages/console/app/script/generate-sitemap.ts and packages/opencode/script/schema.ts.
// Invariants: transformations fail closed on source drift; subprocess HOME/XDG paths stay isolated.
// Contract tests: build-artifacts.test.ts.
import { mkdir, readFile } from "node:fs/promises"
import { resolve, dirname } from "node:path"
import { fileURLToPath } from "node:url"

export const repository = resolve(fileURLToPath(new URL("../../../", import.meta.url)))
export const sitemapSource = resolve(repository,"packages/console/app/script/generate-sitemap.ts")
export const schemaSource = resolve(repository,"packages/opencode/script/schema.ts")

export function isolatedSitemap(source: string, publicDir: string) {
  const replacements = [
    ['const __dirname = dirname(fileURLToPath(import.meta.url))',`const __dirname = ${JSON.stringify(dirname(sitemapSource))}`],
    ['const PUBLIC_DIR = join(__dirname, "../public")',`const PUBLIC_DIR = ${JSON.stringify(publicDir)}`],
    ['"../src/config.js"',JSON.stringify(resolve(dirname(sitemapSource),"../src/config.ts"))],
    ['"../src/lib/language.js"',JSON.stringify(resolve(dirname(sitemapSource),"../src/lib/language.ts"))],
    ['void main()', 'await main()'],
  ]
  return replacements.reduce((text,[before,after])=>{
    if(text.split(before).length!==2)throw new Error(`Sitemap generator source drift: ${before}`)
    return text.replace(before,after)
  },source)
}

export async function runGenerator(args: string[], cwd: string, home: string) {
  const child = Bun.spawn([process.execPath,...args],{
    cwd,env:{...process.env,HOME:home,XDG_CONFIG_HOME:resolve(home,"config"),XDG_DATA_HOME:resolve(home,"data"),XDG_STATE_HOME:resolve(home,"state"),XDG_CACHE_HOME:resolve(home,"cache")},
    stdout:"pipe",stderr:"pipe",
  })
  const [status,stdout,stderr] = await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()])
  if(status!==0)throw new Error(`Build artifact generator failed (${status}): ${stderr}\n${stdout}`)
  return stdout
}

export async function generateBuildArtifacts(publicDir: string, home: string) {
  await mkdir(publicDir,{recursive:true})
  await mkdir(home,{recursive:true})
  const source = isolatedSitemap(await readFile(sitemapSource,"utf8"),publicDir)
  await runGenerator(["--eval",source],resolve(repository,"packages/console/app"),home)
  await runGenerator([schemaSource,resolve(publicDir,"config.json"),resolve(publicDir,"tui.json")],resolve(repository,"packages/opencode"),home)
  const files = await Promise.all(["sitemap.xml","config.json","tui.json"].map(async name=>{
    const file = Bun.file(resolve(publicDir,name))
    const content = await file.text()
    if(!content.length)throw new Error(`Generated asset is empty: ${name}`)
    if(name.endsWith(".json"))JSON.parse(content)
    return {name,bytes:file.size}
  }))
  console.log(`RUST_CONSOLE_PUBLIC_ARTIFACTS ${JSON.stringify(files)}`)
  return files
}
