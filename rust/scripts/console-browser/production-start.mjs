// Purpose: serve built Node SSR and route every native API to the colocated Axum process.
// Scope: loopback-only production ingress behind an explicit TLS terminating proxy.
// Uses: node:http and generated Nitro node-server artifact.
// Invariants: no Cloudflare loopback claim, exact public host, no cloud TS handler fallback.
// Contract tests: production.test.ts and real Chromium production acceptance.
import http from "node:http"
import { spawn } from "node:child_process"
import { fileURLToPath } from "node:url"
import { once } from "node:events"

export const nativePaths = ["/auth","/api","/stripe","/bench/submission","/changelog.json","/github.json","/openapi.json","/oauth","/zen/v1","/zen/go/v1","/workspaces","/_console","/honeycomb","/stats","/data","/docs","/s/","/t/","/download/","/benchmarks","/discord","/feishu","/desktop-feedback"]
export function isNative(pathname) {
  const stripped = pathname.replace(/^\/(?:en|zh|zht|ko|de|es|fr|it|da|ja|pl|ru|uk|ar|no|br|th|tr)(?=\/)/,"")
  return nativePaths.some(path=>stripped===path || stripped.startsWith(path.endsWith("/")?path:path+"/"))
}

export async function startProduction() {
  const backend = new URL(process.env.OPENCODE_BROWSER_RUST_URL ?? "http://127.0.0.1:8787")
  if (backend.protocol!=="http:" || !["127.0.0.1","[::1]","localhost"].includes(backend.hostname) || backend.pathname!=="/" || backend.username || backend.password || backend.search || backend.hash) throw new Error("Rust backend must be a loopback HTTP origin")
  const origin = new URL(process.env.OPENCODE_CONSOLE_PUBLIC_ORIGIN ?? "")
  if (!["http:","https:"].includes(origin.protocol) || origin.username || origin.password || origin.pathname!=="/" || origin.search || origin.hash) throw new Error("Public origin must be exact HTTP(S) origin")
  const port = Number(process.env.OPENCODE_BROWSER_PORT ?? 3001)
  const ssrPort = Number(process.env.OPENCODE_SSR_PORT ?? 3002)
  if (![port,ssrPort].every(value=>Number.isInteger(value)&&value>0&&value<65536) || port===ssrPort) throw new Error("Distinct valid front and SSR ports are required")
  const entry = process.env.OPENCODE_BROWSER_PRODUCTION_ENTRY ?? fileURLToPath(new URL("../../target/console-browser-production/output/server/index.mjs",import.meta.url))
  const child = spawn(process.execPath,[entry],{env:{...process.env,NITRO_HOST:"127.0.0.1",NITRO_PORT:String(ssrPort)},stdio:["ignore","inherit","inherit"]})
  let stopping
  const server = http.createServer((request,response)=>{
    if (request.headers.host!==origin.host) {response.writeHead(421);response.end("Public host mismatch");return}
    if (!request.url?.startsWith("/") || request.url.startsWith("//") || request.url.includes("\\")) {response.writeHead(400);response.end("Invalid request target");return}
    const target=isNative(new URL(request.url,origin).pathname)?backend:new URL(`http://127.0.0.1:${ssrPort}`)
    const upstream=http.request(new URL(request.url,target),{method:request.method,headers:{...request.headers,host:origin.host,"x-forwarded-proto":origin.protocol.slice(0,-1),"x-forwarded-host":origin.host}},result=>{
      response.writeHead(result.statusCode??502,result.headers)
      result.pipe(response)
    })
    upstream.on("error",()=>{if(!response.headersSent)response.writeHead(502);response.end("Native console backend unavailable")})
    request.on("aborted",()=>upstream.destroy())
    request.pipe(upstream)
  })
  const close=()=>stopping??=(async()=>{
    const closed=new Promise(resolve=>server.close(resolve))
    server.closeAllConnections()
    if(child.exitCode===null && child.signalCode===null){
      const exited=once(child,"exit")
      child.kill("SIGTERM")
      const timer=setTimeout(()=>child.kill("SIGKILL"),5000)
      timer.unref()
      await exited
      clearTimeout(timer)
    }
    await closed
  })()
  child.once("exit",()=>{if(!stopping){void close();process.exitCode=1}})
  process.once("SIGTERM",()=>{void close()})
  process.once("SIGINT",()=>{void close()})
  await new Promise((resolve,reject)=>{server.once("error",reject);server.listen(port,"127.0.0.1",resolve)}).catch(async error=>{await close();throw error})
  console.log(`RUST_CONSOLE_BROWSER_READY ${origin.origin}`)
  return {server,child,close}
}

if (process.argv[1]===fileURLToPath(import.meta.url)) await startProduction()
