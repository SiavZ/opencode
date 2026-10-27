// Purpose: verify fail-closed support bridge against real disposable local HTTP servers.
// Invariants: no production endpoints, no credential reflection, no global fetch mocking.
import {expect,test} from "bun:test"
import {configuration,allowed,lookup} from "./runtime"
const secret="support-runtime-canary-not-public"
test("configuration rejects nonliteral remote origins and absent credentials",()=>{
 for(const url of ["https://127.0.0.1:8","http://localhost:8","http://example.test","http://127.0.0.1:8/path","http://u:p@127.0.0.1:8","http://127.0.0.1:8/?x=1"])expect(()=>configuration({OPENCODE_SUPPORT_RUST_URL:url,SUPPORT_API_KEY:secret})).toThrow()
 expect(()=>configuration({OPENCODE_SUPPORT_RUST_URL:"http://127.0.0.1:8"})).toThrow()
 const origin="http://127.0.0.1:3031"
 expect(allowed(new Request(origin,{headers:{host:"127.0.0.1:3031"}}),origin)).toBe(true)
 expect(allowed(new Request(origin,{headers:{host:"evil.test"}}),origin)).toBe(false)
 expect(allowed(new Request(origin,{headers:{host:"127.0.0.1:3031",origin:"https://evil.test"}}),origin)).toBe(false)
})
test("real runtime HTTP lookup and malformed error bodies never reflect key",async()=>{
 const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch(request){
  expect(request.headers.get("authorization")).toBe(`Bearer ${secret}`)
  const id=new URL(request.url).searchParams.get("identifier")
  if(id==="bad")return new Response(`malformed-${secret}`,{status:200})
  if(id==="oversized")return new Response("x".repeat(4*1024*1024+1),{status:200})
  if(id==="missing")return new Response(secret,{status:404})
  if(id==="unauthorized")return new Response(secret,{status:401})
  if(id==="redirect")return Response.redirect(`http://127.0.0.1:${server.port}/steal`)
  if(id==="echo")return Response.json({identifier:id,workspaces:[],echo:secret})
  return Response.json({identifier:id,workspaces:[]})
 }})
 const previous={url:process.env.OPENCODE_SUPPORT_RUST_URL,key:process.env.SUPPORT_API_KEY}
 process.env.OPENCODE_SUPPORT_RUST_URL=`http://127.0.0.1:${server.port}`;process.env.SUPPORT_API_KEY=secret
 try{
  expect(await lookup("wrk_fixture")).toEqual({identifier:"wrk_fixture",workspaces:[]})
  for(const [id,message] of [["bad","Native support response invalid"],["oversized","Native support lookup unavailable"],["missing","Support identifier not found"],["unauthorized","Native support lookup unavailable"],["redirect","Native support lookup unavailable"],["echo","Native support response invalid"]]){
   const error=await lookup(id).then(()=>null,error=>error)
   expect(error.message).toBe(message);expect(String(error)).not.toContain(secret)
  }
 }finally{server.stop(true);if(previous.url===undefined)delete process.env.OPENCODE_SUPPORT_RUST_URL;else process.env.OPENCODE_SUPPORT_RUST_URL=previous.url;if(previous.key===undefined)delete process.env.SUPPORT_API_KEY;else process.env.SUPPORT_API_KEY=previous.key}
})
