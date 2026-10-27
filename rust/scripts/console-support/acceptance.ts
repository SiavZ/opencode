// Purpose: exercise unchanged support lookup UI in real Chromium against local native Rust.
// Scope: disposable fixture only; never contact production or deploy.
// Contract tests: completed by rose's Rust harness using its seeded database.
import { createRequire } from "node:module"
import { resolve } from "node:path"
import { fileURLToPath } from "node:url"
const root=fileURLToPath(new URL("../../../",import.meta.url))
import { spawn } from "node:child_process"
import { start } from "./start.mjs"
import { outputRoot } from "./build"

const secret=process.env.SUPPORT_API_KEY
const native=process.env.OPENCODE_SUPPORT_RUST_URL
const identifier=process.env.OPENCODE_SUPPORT_TEST_IDENTIFIER
const workspace=process.env.OPENCODE_SUPPORT_TEST_WORKSPACE
const name=process.env.OPENCODE_SUPPORT_TEST_NAME??process.env.OPENCODE_SUPPORT_TEST_TEXT
const email=process.env.OPENCODE_SUPPORT_TEST_EMAIL
if(!secret||!native||!identifier||!workspace||!name||!email)throw new Error("Support Chromium fixture environment incomplete")
const backend=new URL(native)
if(backend.protocol!=="http:"||backend.hostname!=="127.0.0.1")throw new Error("Support fixture must use a literal loopback native server")
const nativeSuccess=await fetch(new URL(`/api/support/lookup?identifier=${encodeURIComponent(identifier)}`,backend),{headers:{authorization:`Bearer ${secret}`},redirect:"error"})
const success=await nativeSuccess.text()
if(!nativeSuccess.ok||!success.includes(name))throw new Error("Native support fixture lookup failed")
for(const authorization of [undefined,"Bearer wrong-support-browser-key"]){
 const response=await fetch(new URL(`/api/support/lookup?identifier=${encodeURIComponent(identifier)}`,backend),{headers:authorization?{authorization}:{},redirect:"error"})
 if(response.status!==401)throw new Error("Native support lookup did not reject missing/wrong bearer")
}
const require=createRequire(resolve(root,"packages/console/support/package.json"))
const {chromium}=require(require.resolve("playwright",{paths:[resolve(root,"packages/console/app"),resolve(root,"node_modules/.bun/@playwright+test@1.59.1/node_modules")]}))
const port=Number(process.env.OPENCODE_SUPPORT_PORT??3031)
process.env.OPENCODE_SUPPORT_PORT=String(port)
let service
let browser
let servicePending
let browserPending
let cleaning
const cleanup=()=>cleaning??=(async()=>{
 const resources=await Promise.allSettled([servicePending,browserPending])
 await Promise.all([resources[0].status==="fulfilled"?resources[0].value?.stop():undefined,resources[1].status==="fulfilled"?resources[1].value?.close():undefined])
})()
const terminate=()=>void cleanup().then(()=>process.exit(1),()=>process.exit(1))
process.once("SIGTERM",terminate);process.once("SIGINT",terminate)
try{
 servicePending=start()
 service=await servicePending
 browserPending=chromium.launch({headless:true})
 browser=await browserPending
 const context=await browser.newContext()
 const page=await context.newPage()
 const pageErrors=[]
 const leaks=[]
 const clientErrors=[]
 const scans=[]
 context.on("page",page=>page.on("pageerror",error=>pageErrors.push(error.message)))
 page.on("pageerror",error=>pageErrors.push(error.message))
 context.on("request",request=>{if((request.url()+JSON.stringify(request.headers())+(request.postData()??"")).includes(secret))leaks.push("request")})
 context.on("response",response=>scans.push((async()=>{let body="";try{body=await response.text()}catch{};if(body.includes(secret)||JSON.stringify(response.headers()).includes(secret))leaks.push("response")})()))
 context.on("requestfailed",request=>{if(request.failure()?.errorText!=='net::ERR_ABORTED')clientErrors.push(`${request.url()}:${request.failure()?.errorText}`)})
 await page.goto(`http://127.0.0.1:${port}/`,{waitUntil:"networkidle"})
 if((await page.locator('main[data-page="support"]').count())!==1)throw new Error("Support SSR home missing")
 const home=await page.content()
 if(home.includes(secret))throw new Error("Support canary appeared in SSR HTML")
 await page.locator('form[data-component="lookup"] input[name="identifier"]').fill(identifier)
 const popupPromise=context.waitForEvent("page")
 await page.locator('form[data-component="lookup"] button').click()
 const popup=await popupPromise
 await popup.waitForLoadState("networkidle")
 await popup.locator('main[data-page="support"]').waitFor()
 if(!decodeURIComponent(popup.url()).includes(identifier))throw new Error("Lookup popup query missing identifier")
 const section=popup.locator('section[data-component="section"]').filter({has:popup.getByRole("heading",{name:`Workspace "${name}" (${workspace})`,exact:true,level:2})})
 await section.waitFor({state:"visible"})
 if(await section.count()!==1||await section.getAttribute("id")!==`workspace-${workspace}`)throw new Error("Seeded workspace section not unique")
 for(const heading of ["Users","Billing","GO","Payments","28-Day Usage","Disabled Models"])await section.getByRole("heading",{name:heading,exact:true,level:3}).waitFor({state:"visible"})
 const users=section.getByRole("heading",{name:"Users",exact:true,level:3}).locator("xpath=following-sibling::table[1]")
 const userEmail=users.getByRole("cell",{name:email,exact:true})
 await userEmail.waitFor({state:"visible"})
 if(await userEmail.count()!==1)throw new Error("Seeded user email cell not unique")
 const resultHtml=await popup.content()
 if(resultHtml.includes(secret))throw new Error("Support canary appeared in lookup SSR HTML")
 for(const asset of await popup.locator('script[src],link[rel="stylesheet"]').evaluateAll(nodes=>nodes.map(node=>node.getAttribute("src")??node.getAttribute("href")).filter(Boolean))){
  const url=new URL(asset,popup.url())
  const response=await fetch(url)
  const body=await response.text()
  if(!response.ok||body.includes(secret))throw new Error("Support asset failed or exposed canary")
 }
 const hostDenied=await fetch(`http://127.0.0.1:${port}/`,{headers:{host:"127.0.0.1:1"}})
 const originDenied=await fetch(`http://127.0.0.1:${port}/`,{headers:{origin:"http://evil.invalid"}})
 const siteDenied=await fetch(`http://127.0.0.1:${port}/`,{headers:{"sec-fetch-site":"cross-site"}})
 if(hostDenied.status!==421||originDenied.status!==403||siteDenied.status!==403)throw new Error("Support loopback ingress did not reject hostile headers")
 const ssr=`http://127.0.0.1:${service.ssrPort}`
 for(const headers of [{host:"127.0.0.1:1"},{origin:"http://evil.invalid"},{"sec-fetch-site":"cross-site"},{"x-forwarded-proto":"https"}]){
  const response=await fetch(ssr+"/",{headers,redirect:"error"})
  if(response.status!==403)throw new Error("Direct SSR hostile ingress was not denied")
 }
 await Promise.all(scans)
 service.assertLogsClean()
 if(pageErrors.length)throw new Error("Chromium page error")
 if(clientErrors.length)throw new Error("Chromium network failure")
 if(leaks.length)throw new Error("Support canary appeared in browser traffic")
 await context.close()
 console.log("SUPPORT_CHROMIUM_LOOKUP_PASSED")
}finally{
 await cleanup()
 process.off("SIGTERM",terminate);process.off("SIGINT",terminate)
 service?.assertLogsClean()
}
const files=await Array.fromAsync(new Bun.Glob("**/*").scan(outputRoot))
for(const file of files){
 const path=resolve(outputRoot,file)
 if(!(await Bun.file(path).stat()).isFile())continue
 const text=await Bun.file(path).text()
 if(text.includes(secret))throw new Error("Support canary found in generated artifact")
 if((file.startsWith("client/")||file.startsWith("output/public/"))&&/SUPPORT_API_KEY|OPENCODE_SUPPORT_RUST_URL|Native support lookup|drizzle-orm|mysql2|sst\/resource/.test(text))throw new Error("Server runtime or database dependency found in client artifact")
}
const rejected=spawn("node",[resolve(import.meta.dir,"start.mjs")],{env:{...process.env,OPENCODE_SUPPORT_RUST_URL:"http://example.invalid/",SUPPORT_API_KEY:secret,OPENCODE_SUPPORT_PORT:String(port+1)},stdio:["ignore","pipe","pipe"]})
const code=await new Promise(resolve=>{
 const timeout=setTimeout(()=>rejected.kill("SIGKILL"),3000)
 rejected.once("error",()=>{clearTimeout(timeout);resolve(null)})
 rejected.once("exit",code=>{clearTimeout(timeout);resolve(code)})
})
if(code!==1)throw new Error("Remote backend configuration did not fail closed")
console.log("SUPPORT_ARTIFACT_CANARY_SCAN_PASSED")
