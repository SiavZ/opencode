// Purpose: replace only the unchanged support lookup server query with native transport.
// Scope: in-memory pre-SolidStart compiler transform; baseline files remain read-only.
// Invariants: unexpected baseline syntax fails closed; direct DB runtime import is removed.
// Contract tests: plugin.test.ts.
import { createRequire } from "node:module"
import { resolve } from "node:path"
import { fileURLToPath } from "node:url"
export const supportRoot = fileURLToPath(new URL("../../../packages/console/support/", import.meta.url))
const require = createRequire(resolve(supportRoot, "package.json"))
const framework = createRequire(require.resolve("@solidjs/start/config"))
const babel = framework("@babel/core")
const runtime = fileURLToPath(new URL("./runtime.ts", import.meta.url))
export function rewrite(code: string, id: string) {
  if (id.split("?")[0] !== resolve(supportRoot, "src/routes/lookup.tsx")) return
  const ast = babel.parseSync(code, { filename: id.split("?")[0], parserOpts: { plugins: ["typescript", "jsx"] }, configFile: false, babelrc: false })
  const edits: { start: number; end: number; text: string }[] = []
  babel.traverse(ast, {
    VariableDeclarator(path) {
      if (path.node.id.name !== "getLookup") return
      const call = path.node.init
      const fn = call?.arguments?.[0]
      if (call?.callee?.name !== "query" || call.arguments.length !== 2 || call.arguments[1].value !== "support.lookup" || fn?.type !== "ArrowFunctionExpression" || !fn.async || fn.params.length !== 1 || fn.params[0].name !== "identifier" || !fn.body.directives?.some((directive) => directive.value.value === "use server")) throw new Error("Baseline support query changed")
      const body = fn.body.body
      if (body.length !== 1 || body[0].type !== "ReturnStatement" || body[0].argument?.callee?.name !== "lookup" || body[0].argument.arguments.length !== 1 || body[0].argument.arguments[0].name !== "identifier") throw new Error("Baseline support lookup body changed")
      edits.push({ start: fn.body.start, end: fn.body.end, text: '{ "use server"; return __nativeSupportLookup(identifier); }' })
    },
    ImportDeclaration(path) {
      if (path.node.source.value !== "~/lib/lookup") return
      if (path.node.specifiers.length !== 1 || path.node.specifiers[0].imported?.name !== "lookup" || path.node.specifiers[0].local.name !== "lookup") throw new Error("Baseline support lookup import changed")
      edits.push({ start: path.node.start, end: path.node.end, text: "" })
    },
  })
  if (edits.length !== 2) throw new Error("Exact support lookup query/import required")
  return { code: `import { lookup as __nativeSupportLookup } from ${JSON.stringify(runtime)};\n` + edits.sort((a,b) => b.start-a.start).reduce((output, edit) => output.slice(0,edit.start)+edit.text+output.slice(edit.end),code), map: null }
}
export function supportBridge() {
  return { name: "native-operator-support", enforce: "pre" as const, transform: { order: "pre" as const, handler: rewrite } }
}