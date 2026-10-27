// Purpose: compile unchanged console server actions to the Rust browser boundary in memory.
// Scope: local Vite plugin, no baseline writes or cloud fallback.
// Uses: installed SolidStart Babel compiler and runtime.ts.
// Invariants: real SolidStart still generates IDs and owns Seroval, forms, SSR and redirects.
// Contract tests: acceptance.test.ts.
import { createRequire } from "node:module"
import { fileURLToPath } from "node:url"
import { resolve } from "node:path"
import { readFileSync } from "node:fs"

const app = fileURLToPath(new URL("../../../packages/console/app/", import.meta.url))
const require = createRequire(resolve(app, "package.json"))
const babel = require(createRequire(require.resolve("@solidjs/start/config")).resolve("@babel/core"))
const runtime = fileURLToPath(new URL("./runtime.ts", import.meta.url))

export function rewrite(code: string, id: string) {
  if (!id.startsWith(resolve(app, "src")) || !/\.[jt]sx?(?:\?|$)/.test(id)) return
  if (id.split("?")[0] === resolve(app, "src/lib/referral-invite.ts")) {
    const baseline = readFileSync(resolve(app, "../core/src/referral.ts"), "utf8")
    const parsed = babel.parseSync(baseline, { parserOpts: { plugins: ["typescript"] }, configFile: false, babelrc: false })
    let body = ""
    babel.traverse(parsed, { FunctionDeclaration(path) { if (path.node.id?.name === "normalizeCode") body = baseline.slice(path.node.body.start, path.node.body.end) } })
    if (!body) throw new Error("Baseline referral normalizer missing")
    const length = /export const CODE_LENGTH = (\d+)/.exec(baseline)?.[1]
    if (!length) throw new Error("Baseline referral code length missing")
    code = code.replace(/return Referral\.normalizeCode\(code\)/, body.slice(1, -1).replaceAll("CODE_LENGTH", length))
    const parsedInvite = babel.parseSync(code, { parserOpts: { plugins: ["typescript"] }, configFile: false, babelrc: false })
    babel.traverse(parsedInvite, { FunctionDeclaration(path) { if (path.node.id?.name === "createReferralFromCookie") code = code.slice(0, path.node.body.start) + '{ "use server"; return __rustConsoleAction("go.referral.capture", []); }' + code.slice(path.node.body.end) } })
  }
  if (!code.includes('"use server"')) return
  const ast = babel.parseSync(code, { filename: id.split("?")[0], parserOpts: { plugins: ["typescript", "jsx"] }, configFile: false, babelrc: false })
  const edits: { start: number; end: number; text: string }[] = []
  babel.traverse(ast, {
    Function(path) {
      if (!path.node.body.directives?.some((d) => d.value.value === "use server")) return
      const wrapper = path.parentPath
      const operation = wrapper.isCallExpression() && ["query", "action"].includes(wrapper.node.callee.name)
        ? wrapper.node.arguments[1]?.value ?? wrapper.findParent((p) => p.isVariableDeclarator())?.node.id.name
        : path.node.id?.name ?? (wrapper.isVariableDeclarator() ? wrapper.node.id.name : undefined)
      const mapped = operation === "createReferralFromCookie" ? "go.referral.capture" : operation === "getLastSeenWorkspaceID" ? "lastWorkspace"
        : operation === "getUsageInfo" ? "usage.list" : operation ?? "unsupported"
      const params = path.node.params.map((p) => {
        if (p.type === "Identifier") return p.name
        if (p.type === "AssignmentPattern" && p.left.type === "Identifier") return p.left.name
        throw new Error(`Unsupported server argument pattern in ${id}`)
      })
      edits.push({ start: path.node.body.start, end: path.node.body.end, text: `{ "use server"; return __rustConsoleAction(${JSON.stringify(mapped)}, [${params.join(",")}]); }` })
      path.skip()
    },
  })
  if (!edits.length) return
  const rewritten = edits.sort((a, b) => b.start - a.start).reduce((text, edit) => text.slice(0, edit.start) + edit.text + text.slice(edit.end), code)
  const next = babel.parseSync(`import { invoke as __rustConsoleAction } from ${JSON.stringify(runtime)};\n${rewritten}`, { filename: id.split("?")[0], parserOpts: { plugins: ["typescript", "jsx"] }, configFile: false, babelrc: false })
  // Vite dev executes even unused imports. Remove only bindings no longer referenced after
  // replacing server bodies, so database/Stripe modules cannot run as an accidental fallback.
  babel.traverse(next, { Program(path) {
    path.scope.crawl()
    path.get("body").filter((p) => p.isImportDeclaration()).forEach((p) => {
      if (!p.node.specifiers.length) return
      p.get("specifiers").forEach((s) => {
        const binding = path.scope.getBinding(s.node.local.name)
        const runtimeReference = binding?.referencePaths.some((reference) => {
          for (let parent = reference; parent && parent !== path; parent = parent.parentPath) {
            if (parent.node.type.startsWith("TS") && !["TSAsExpression", "TSNonNullExpression", "TSSatisfiesExpression"].includes(parent.node.type)) return false
          }
          return true
        })
        if (!runtimeReference) s.remove()
      })
      if (!p.node.specifiers.length) p.remove()
    })
  } })
  return babel.transformFromAstSync(next, undefined, { filename: id.split("?")[0], configFile: false, babelrc: false, ast: false, code: true, sourceMaps: true })
}

export function rustConsole() {
  return {
    name: "rust-console-server-actions",
    enforce: "pre" as const,
    transform: { order: "pre" as const, handler(code: string, id: string) { return rewrite(code, id) } },
  }
}

export const appRoot = app