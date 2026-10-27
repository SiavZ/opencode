// Purpose: verify exact unchanged support query rewrite and compiler isolation.
// Contract tests: this file, plus acceptance.ts compiled browser checks.
import {expect,test} from "bun:test"
import {resolve} from "node:path"
import {rewrite,supportRoot} from "./plugin"
test("only exact baseline support query changes and source stays untouched",async()=>{
 const path=resolve(supportRoot,"src/routes/lookup.tsx")
 const baseline=await Bun.file(path).text()
 const output=rewrite(baseline,path)!.code
 expect(output).toContain('return __nativeSupportLookup(identifier)')
 expect(output).toContain('support.lookup')
 expect(output).not.toContain('from "~/lib/lookup"')
 expect(output).not.toContain('console-core')
 expect(output).not.toContain('SUPPORT_API_KEY')
 expect(await Bun.file(path).text()).toBe(baseline)
 expect(()=>rewrite(baseline.replace('return lookup(identifier)','return lookup(identifier, 1)'),path)).toThrow()
 expect(()=>rewrite(baseline.replace('support.lookup','support.changed'),path)).toThrow()
 expect(rewrite(baseline,resolve(supportRoot,"src/component/result.tsx"))).toBeUndefined()
})
