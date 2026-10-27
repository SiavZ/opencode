// Purpose: reject unrelated cloud golden deltas before approved native-console regeneration.
// Scope: read-only comparison of three generated plans against checked-in cloud goldens.
// Uses: Bun.file JSON reader and console native topology resource names.
// Invariants: never writes; any change outside explicit console resources fails closed.
// Contract tests: xtask cloud tests and this command before cloud golden --write.
import { resolve } from "node:path"

const root=resolve(import.meta.dir,"../..")
const names=["AuthApi","AuthStorage","Console","LogProcessor","Stat"]
const native=[...names,"NativeConsoleTopology"]
const secrets=["OPENCODE_ISSUER_SIGNING_KEY_PEM","OPENCODE_ISSUER_RETIRED_JWKS","OPENCODE_ISSUER_KEY_ID","OPENCODE_CONSOLE_LOG_INGRESS_TOKEN"]
for(const stage of ["production","dev","pr-123"]){
  const old=await Bun.file(resolve(root,"contracts/phase-13/cloud-golden",`${stage}.tf.json`)).json()
  const next=await Bun.file(resolve(root,"target/native-console-plan",stage,"main.tf.json")).json()
  for(const name of names){delete old.module[name];delete next.module[name]}
  delete old.resource.cloudflare_workers_kv_namespace?.AuthStorage
  delete next.resource.cloudflare_workers_kv_namespace?.AuthStorage
  if(old.resource.cloudflare_workers_kv_namespace && !Object.keys(old.resource.cloudflare_workers_kv_namespace).length)delete old.resource.cloudflare_workers_kv_namespace
  if(next.resource.cloudflare_workers_kv_namespace && !Object.keys(next.resource.cloudflare_workers_kv_namespace).length)delete next.resource.cloudflare_workers_kv_namespace
  for(const name of native){delete old.locals[name];delete next.locals[name]}
  for(const name of secrets){delete old.variable[name];delete next.variable[name]}
  delete old.locals.AUTH_API_URL;delete next.locals.AUTH_API_URL
  delete old.output.StatWorkerUrl;delete next.output.StatWorkerUrl
  if(JSON.stringify(old)!==JSON.stringify(next))throw new Error(`Unrelated cloud golden delta for ${stage}; regeneration forbidden`)
  console.log(`NATIVE_CONSOLE_GOLDEN_DELTA_ONLY ${stage}`)
}