# Rust port baseline coverage inventory

This is a baseline registry inventory, not implementation approval.
`unverified` means no end-to-end parity claim has been established for that row.
`not-ported` means its happy-path HTTP case is recorded as `not-mounted` or
`not-ported` in the versioned server replay ledger. The ledger can become stale
as implementation changes; differential replay must validate it independently.
No row here is a release-readiness signoff. CLI parsing/help does not establish
command execution, and provider registration does not establish a live model turn.

The inventory includes exported OpenAPI operations, current and legacy TypeScript
HTTP declarations, built-in tool definitions, CLI command literals, event
definitions and bundled provider adapters. Dynamic user/plugin/MCP registrations
are outside the static baseline, not counted as implemented. HTTP declarations
are retained separately so SDK generation cannot conceal a new TS route.

Regenerate the baseline map explicitly with `cargo xtask inventory --write`.
The read-only `inventory` gate rejects missing, new or stale rows and unknown IDs.
Generated IDs identify source declarations; their presence proves enumeration only.

| Coverage ID | Registry | Name | Baseline source | Status |
| --- | --- | --- | --- | --- |
| BASELINE-CLI-db595623ae07 | cli | `$0 [project]` | `packages/opencode/src/cli/cmd/tui.ts:73` | unverified |
| BASELINE-CLI-7261600c82df | cli | `$0 [query]` | `packages/opencode/src/cli/cmd/db.ts:9` | unverified |
| BASELINE-CLI-4152a2f40236 | cli | `, () =>
        Promise.all([proc.exited, text(proc.stdout!)]),
      ).pipe(Effect.ensuring(Effect.sync(() => abort.abort())))
      if (exit !== 0) {
        yield* Prompt.log.error(` | `packages/opencode/src/cli/cmd/providers.ts:340` | unverified |
| BASELINE-CLI-8e9f8ef36494 | cli | `acp` | `packages/opencode/src/cli/cmd/acp.ts:10` | unverified |
| BASELINE-CLI-82ac839aae78 | cli | `add [name]` | `packages/opencode/src/cli/cmd/mcp.ts:430` | unverified |
| BASELINE-CLI-c225058b3003 | cli | `agent` | `packages/opencode/src/cli/cmd/agent.ts:255` | unverified |
| BASELINE-CLI-1bb0207734e2 | cli | `agent <name>` | `packages/opencode/src/cli/cmd/debug/agent.ts:5` | unverified |
| BASELINE-CLI-502c213e7f52 | cli | `attach <url>` | `packages/opencode/src/cli/cmd/attach.ts:8` | unverified |
| BASELINE-CLI-defcff1df933 | cli | `auth [name]` | `packages/opencode/src/cli/cmd/mcp.ts:171` | unverified |
| BASELINE-CLI-8c3fa5ddc0a1 | cli | `config` | `packages/opencode/src/cli/cmd/debug/config.ts:7` | unverified |
| BASELINE-CLI-16f1872e2e59 | cli | `console` | `packages/opencode/src/cli/cmd/account.ts:238` | unverified |
| BASELINE-CLI-aa59d6af86b3 | cli | `create` | `packages/opencode/src/cli/cmd/agent.ts:34` | unverified |
| BASELINE-CLI-86c41c99ef1b | cli | `db` | `packages/opencode/src/cli/cmd/db.ts:55` | unverified |
| BASELINE-CLI-f5fdde3796d2 | cli | `debug` | `packages/opencode/src/cli/cmd/debug/index.ts:20` | unverified |
| BASELINE-CLI-134717b7a665 | cli | `debug <name>` | `packages/opencode/src/cli/cmd/mcp.ts:660` | unverified |
| BASELINE-CLI-4216d9b1af34 | cli | `delete <sessionID>` | `packages/opencode/src/cli/cmd/session.ts:52` | unverified |
| BASELINE-CLI-e606b546a0d2 | cli | `diagnostics <file>` | `packages/opencode/src/cli/cmd/debug/lsp.ts:16` | unverified |
| BASELINE-CLI-9a1bd60cd9fe | cli | `diff <hash>` | `packages/opencode/src/cli/cmd/debug/snapshot.ts:38` | unverified |
| BASELINE-CLI-13b659c6a66c | cli | `document-symbols <uri>` | `packages/opencode/src/cli/cmd/debug/lsp.ts:42` | unverified |
| BASELINE-CLI-697cf0f175a8 | cli | `export [sessionID]` | `packages/opencode/src/cli/cmd/export.ts:223` | unverified |
| BASELINE-CLI-26f9be75e5ef | cli | `file` | `packages/opencode/src/cli/cmd/debug/file.ts:68` | unverified |
| BASELINE-CLI-62f6b9226674 | cli | `files` | `packages/opencode/src/cli/cmd/debug/ripgrep.ts:16` | unverified |
| BASELINE-CLI-9e20134eb605 | cli | `generate` | `packages/opencode/src/cli/cmd/generate.ts:6` | unverified |
| BASELINE-CLI-87f8c751bd81 | cli | `git status` | `packages/opencode/src/cli/cmd/run/demo.ts:587` | unverified |
| BASELINE-CLI-da9629c3e6fd | cli | `github` | `packages/opencode/src/cli/cmd/github.ts:38` | unverified |
| BASELINE-CLI-4a05c01efa71 | cli | `import <file>` | `packages/opencode/src/cli/cmd/import.ts:95` | unverified |
| BASELINE-CLI-9b1d2fdb1cf4 | cli | `info` | `packages/opencode/src/cli/cmd/debug/index.ts:50` | unverified |
| BASELINE-CLI-7939eb42a251 | cli | `install` | `packages/opencode/src/cli/cmd/github.ts:8` | unverified |
| BASELINE-CLI-e8bcbc4b681a | cli | `list` | `packages/opencode/src/cli/cmd/agent.ts:235` | unverified |
| BASELINE-CLI-586ac22f6b97 | cli | `list` | `packages/opencode/src/cli/cmd/mcp.ts:110` | unverified |
| BASELINE-CLI-aee11e3cdaa8 | cli | `list` | `packages/opencode/src/cli/cmd/mcp.ts:307` | unverified |
| BASELINE-CLI-c6ac60c298f7 | cli | `list` | `packages/opencode/src/cli/cmd/providers.ts:249` | unverified |
| BASELINE-CLI-e781b7453421 | cli | `list` | `packages/opencode/src/cli/cmd/session.ts:71` | unverified |
| BASELINE-CLI-4afbe97a044a | cli | `list <path>` | `packages/opencode/src/cli/cmd/debug/file.ts:53` | unverified |
| BASELINE-CLI-4091ae305b7a | cli | `login [url]` | `packages/opencode/src/cli/cmd/account.ts:178` | unverified |
| BASELINE-CLI-f82b2eecde45 | cli | `login [url]` | `packages/opencode/src/cli/cmd/providers.ts:300` | unverified |
| BASELINE-CLI-e8dd3d03efb4 | cli | `logout [email]` | `packages/opencode/src/cli/cmd/account.ts:193` | unverified |
| BASELINE-CLI-b24af7606e28 | cli | `logout [name]` | `packages/opencode/src/cli/cmd/mcp.ts:337` | unverified |
| BASELINE-CLI-0f6c4d24a4f7 | cli | `logout [provider]` | `packages/opencode/src/cli/cmd/providers.ts:492` | unverified |
| BASELINE-CLI-854febceb160 | cli | `lsp` | `packages/opencode/src/cli/cmd/debug/lsp.ts:8` | unverified |
| BASELINE-CLI-fd5bb6ba6ed8 | cli | `mcp` | `packages/opencode/src/cli/cmd/mcp.ts:96` | unverified |
| BASELINE-CLI-b054cb5a6a98 | cli | `models [provider]` | `packages/opencode/src/cli/cmd/models.ts:9` | unverified |
| BASELINE-CLI-036b0c3e79e5 | cli | `open` | `packages/opencode/src/cli/cmd/account.ts:228` | unverified |
| BASELINE-CLI-e2f06eaabfc5 | cli | `orgs` | `packages/opencode/src/cli/cmd/account.ts:218` | unverified |
| BASELINE-CLI-07de4fae0b83 | cli | `patch <hash>` | `packages/opencode/src/cli/cmd/debug/snapshot.ts:23` | unverified |
| BASELINE-CLI-56ffb49a8e6b | cli | `path` | `packages/opencode/src/cli/cmd/db.ts:46` | unverified |
| BASELINE-CLI-5a9f88202ac4 | cli | `paths` | `packages/opencode/src/cli/cmd/debug/index.ts:80` | unverified |
| BASELINE-CLI-87a9d8d37393 | cli | `plugin <module>` | `packages/opencode/src/cli/cmd/plug.ts:179` | unverified |
| BASELINE-CLI-6e3c1a6389b7 | cli | `pr <number>` | `packages/opencode/src/cli/cmd/pr.ts:9` | unverified |
| BASELINE-CLI-58c6579cbe4c | cli | `providers` | `packages/opencode/src/cli/cmd/providers.ts:240` | unverified |
| BASELINE-CLI-6fe5829605c7 | cli | `read <path>` | `packages/opencode/src/cli/cmd/debug/file.ts:32` | unverified |
| BASELINE-CLI-25f7f083b54f | cli | `rg` | `packages/opencode/src/cli/cmd/debug/ripgrep.ts:9` | unverified |
| BASELINE-CLI-fa5446d7008b | cli | `run` | `packages/opencode/src/cli/cmd/github.ts:18` | unverified |
| BASELINE-CLI-a5555eef8274 | cli | `run [message..]` | `packages/opencode/src/cli/cmd/run.ts:127` | unverified |
| BASELINE-CLI-04c9fe6df0ae | cli | `scrap` | `packages/opencode/src/cli/cmd/debug/scrap.ts:5` | unverified |
| BASELINE-CLI-25cb671fabc3 | cli | `search <pattern>` | `packages/opencode/src/cli/cmd/debug/ripgrep.ts:48` | unverified |
| BASELINE-CLI-fa7b6c03db35 | cli | `search <query>` | `packages/opencode/src/cli/cmd/debug/file.ts:17` | unverified |
| BASELINE-CLI-d7df91d6ab9f | cli | `serve` | `packages/opencode/src/cli/cmd/serve.ts:7` | unverified |
| BASELINE-CLI-28986cc08e05 | cli | `session` | `packages/opencode/src/cli/cmd/session.ts:45` | unverified |
| BASELINE-CLI-f949c2cc5304 | cli | `skill` | `packages/opencode/src/cli/cmd/debug/skill.ts:7` | unverified |
| BASELINE-CLI-15ad6fe1df36 | cli | `snapshot` | `packages/opencode/src/cli/cmd/debug/snapshot.ts:7` | unverified |
| BASELINE-CLI-d2b7a9527f67 | cli | `startup` | `packages/opencode/src/cli/cmd/debug/startup.ts:5` | unverified |
| BASELINE-CLI-06d81f790ec1 | cli | `stats` | `packages/opencode/src/cli/cmd/stats.ts:50` | unverified |
| BASELINE-CLI-51dfa6aa15ba | cli | `switch` | `packages/opencode/src/cli/cmd/account.ts:208` | unverified |
| BASELINE-CLI-c7bfe6769039 | cli | `symbols <query>` | `packages/opencode/src/cli/cmd/debug/lsp.ts:31` | unverified |
| BASELINE-CLI-30183dbba7da | cli | `track` | `packages/opencode/src/cli/cmd/debug/snapshot.ts:14` | unverified |
| BASELINE-CLI-a805be3039b8 | cli | `uninstall` | `packages/opencode/src/cli/cmd/uninstall.ts:26` | unverified |
| BASELINE-CLI-027e776bcf79 | cli | `upgrade [target]` | `packages/opencode/src/cli/cmd/upgrade.ts:8` | unverified |
| BASELINE-CLI-ec2db6d2d73d | cli | `v2` | `packages/opencode/src/cli/cmd/debug/v2.ts:10` | unverified |
| BASELINE-CLI-9f4bdec6e714 | cli | `wait` | `packages/opencode/src/cli/cmd/debug/index.ts:42` | unverified |
| BASELINE-CLI-5c33e32863e2 | cli | `web` | `packages/opencode/src/cli/cmd/web.ts:32` | unverified |
| BASELINE-EVENT-d1eef94f8afd | event | `global.disposed` | `packages/schema/src/server-event.ts:6` | unverified |
| BASELINE-EVENT-9c9e04c3cbba | event | `ide.installed` | `packages/schema/src/ide-event.ts:7` | unverified |
| BASELINE-EVENT-1289474ad14f | event | `installation.update-available` | `packages/schema/src/installation-event.ts:14` | unverified |
| BASELINE-EVENT-dfb32add0320 | event | `installation.updated` | `packages/schema/src/installation-event.ts:7` | unverified |
| BASELINE-EVENT-ed837ceb42d3 | event | `lsp.updated` | `packages/schema/src/lsp-event.ts:5` | unverified |
| BASELINE-EVENT-de9cbabd8ae9 | event | `mcp.browser.open.failed` | `packages/schema/src/mcp-event.ts:14` | unverified |
| BASELINE-EVENT-2637c6905824 | event | `mcp.tools.changed` | `packages/schema/src/mcp-event.ts:7` | unverified |
| BASELINE-EVENT-c7b878130f04 | event | `server.connected` | `packages/schema/src/server-event.ts:5` | unverified |
| BASELINE-EVENT-c067755b8c7c | event | `session.compacted` | `packages/schema/src/session-compaction-event.ts:7` | unverified |
| BASELINE-EVENT-0845371fba3d | event | `session.idle` | `packages/schema/src/session-status-event.ts:45` | unverified |
| BASELINE-EVENT-5880214d0126 | event | `session.next.agent.switched` | `packages/schema/src/session-event.ts:55` | unverified |
| BASELINE-EVENT-b3e3746b6906 | event | `session.next.compaction.delta` | `packages/schema/src/session-event.ts:411` | unverified |
| BASELINE-EVENT-e2e272447c7c | event | `session.next.compaction.ended` | `packages/schema/src/session-event.ts:421` | unverified |
| BASELINE-EVENT-769dc899be55 | event | `session.next.compaction.started` | `packages/schema/src/session-event.ts:400` | unverified |
| BASELINE-EVENT-915dd91a70a2 | event | `session.next.context.updated` | `packages/schema/src/session-event.ts:102` | unverified |
| BASELINE-EVENT-71a06380e713 | event | `session.next.model.switched` | `packages/schema/src/session-event.ts:66` | unverified |
| BASELINE-EVENT-7c13d7ee48ec | event | `session.next.moved` | `packages/schema/src/session-event.ts:77` | unverified |
| BASELINE-EVENT-64a9547b8069 | event | `session.next.prompt.admitted` | `packages/schema/src/session-event.ts:95` | unverified |
| BASELINE-EVENT-f49a0390a451 | event | `session.next.prompted` | `packages/schema/src/session-event.ts:88` | unverified |
| BASELINE-EVENT-da656a5025a7 | event | `session.next.reasoning.delta` | `packages/schema/src/session-event.ts:249` | unverified |
| BASELINE-EVENT-c608d922b36b | event | `session.next.reasoning.ended` | `packages/schema/src/session-event.ts:260` | unverified |
| BASELINE-EVENT-cf8ab68cfb4a | event | `session.next.reasoning.started` | `packages/schema/src/session-event.ts:236` | unverified |
| BASELINE-EVENT-a1edfe6d3c1f | event | `session.next.retried` | `packages/schema/src/session-event.ts:388` | unverified |
| BASELINE-EVENT-b13e6085dddd | event | `session.next.revert.cleared` | `packages/schema/src/session-event.ts:440` | unverified |
| BASELINE-EVENT-4e65ed0ae165 | event | `session.next.revert.committed` | `packages/schema/src/session-event.ts:442` | unverified |
| BASELINE-EVENT-eeda208fce8a | event | `session.next.revert.staged` | `packages/schema/src/session-event.ts:436` | unverified |
| BASELINE-EVENT-05515ee4de8a | event | `session.next.shell.ended` | `packages/schema/src/session-event.ts:137` | unverified |
| BASELINE-EVENT-95eced4c7121 | event | `session.next.shell.started` | `packages/schema/src/session-event.ts:125` | unverified |
| BASELINE-EVENT-ab75da681689 | event | `session.next.step.ended` | `packages/schema/src/session-event.ts:163` | unverified |
| BASELINE-EVENT-f425460c64ff | event | `session.next.step.failed` | `packages/schema/src/session-event.ts:186` | unverified |
| BASELINE-EVENT-5ea2a71b4296 | event | `session.next.step.started` | `packages/schema/src/session-event.ts:150` | unverified |
| BASELINE-EVENT-042c85c269e6 | event | `session.next.synthetic` | `packages/schema/src/session-event.ts:113` | unverified |
| BASELINE-EVENT-a1e2d96a7b8c | event | `session.next.text.delta` | `packages/schema/src/session-event.ts:211` | unverified |
| BASELINE-EVENT-3efdb6cf6605 | event | `session.next.text.ended` | `packages/schema/src/session-event.ts:222` | unverified |
| BASELINE-EVENT-d54db603985e | event | `session.next.text.started` | `packages/schema/src/session-event.ts:199` | unverified |
| BASELINE-EVENT-d3068d807469 | event | `session.next.tool.called` | `packages/schema/src/session-event.ts:313` | unverified |
| BASELINE-EVENT-500aa9648bb2 | event | `session.next.tool.failed` | `packages/schema/src/session-event.ts:360` | unverified |
| BASELINE-EVENT-b403151f6bae | event | `session.next.tool.input.delta` | `packages/schema/src/session-event.ts:293` | unverified |
| BASELINE-EVENT-f330af297329 | event | `session.next.tool.input.ended` | `packages/schema/src/session-event.ts:302` | unverified |
| BASELINE-EVENT-77f7e14ad856 | event | `session.next.tool.input.started` | `packages/schema/src/session-event.ts:282` | unverified |
| BASELINE-EVENT-1d0970bfa322 | event | `session.next.tool.progress` | `packages/schema/src/session-event.ts:332` | unverified |
| BASELINE-EVENT-a3ec35e8bba2 | event | `session.next.tool.success` | `packages/schema/src/session-event.ts:343` | unverified |
| BASELINE-EVENT-482aec131a70 | event | `session.status` | `packages/schema/src/session-status-event.ts:36` | unverified |
| BASELINE-EVENT-22a0c4d6313d | event | `tui.command.execute` | `packages/schema/src/tui-event.ts:14` | unverified |
| BASELINE-EVENT-f2167bc18c19 | event | `tui.prompt.append` | `packages/schema/src/tui-event.ts:11` | unverified |
| BASELINE-EVENT-e09cd70e66dc | event | `tui.session.select` | `packages/schema/src/tui-event.ts:53` | unverified |
| BASELINE-EVENT-17af1facb57b | event | `tui.toast.show` | `packages/schema/src/tui-event.ts:41` | unverified |
| BASELINE-EVENT-f79a3d701266 | event | `vcs.branch.updated` | `packages/schema/src/vcs-event.ts:8` | unverified |
| BASELINE-EVENT-a000d3d985f9 | event | `workspace.failed` | `packages/schema/src/workspace-event.ts:21` | unverified |
| BASELINE-EVENT-90c1fafe43e7 | event | `workspace.ready` | `packages/schema/src/workspace-event.ts:14` | unverified |
| BASELINE-EVENT-b58314d7dd80 | event | `workspace.status` | `packages/schema/src/workspace-event.ts:28` | unverified |
| BASELINE-EVENT-5b22e3b461a5 | event | `worktree.failed` | `packages/schema/src/worktree-event.ts:16` | unverified |
| BASELINE-EVENT-4153fa247cb4 | event | `worktree.ready` | `packages/schema/src/worktree-event.ts:8` | unverified |
| BASELINE-HTTP-816c5d993fbf | http | `DELETE /api/credential/{credentialID} (v2.credential.remove)` | `packages/sdk/openapi.json:12665` | not-ported |
| BASELINE-HTTP-560e68b6c5e0 | http | `DELETE /api/integration/attempt/{attemptID} (v2.integration.attempt.cancel)` | `packages/sdk/openapi.json:12420` | not-ported |
| BASELINE-HTTP-4fcea4f50e98 | http | `DELETE /api/permission/saved/{id} (v2.permission.saved.remove)` | `packages/sdk/openapi.json:12882` | not-ported |
| BASELINE-HTTP-216bbe48906f | http | `DELETE /api/pty/{ptyID} (v2.pty.remove)` | `packages/sdk/openapi.json:14230` | not-ported |
| BASELINE-HTTP-f003fe74a268 | http | `DELETE /auth/{providerID} (auth.remove)` | `packages/sdk/openapi.json:74` | not-ported |
| BASELINE-HTTP-66234bf11266 | http | `DELETE /experimental/project/{projectID}/copy (v2.projectCopy.remove)` | `packages/sdk/openapi.json:15031` | not-ported |
| BASELINE-HTTP-3a0d4ce4dd0c | http | `DELETE /experimental/workspace/{id} (experimental.workspace.remove)` | `packages/sdk/openapi.json:9617` | not-ported |
| BASELINE-HTTP-38d052fb7318 | http | `DELETE /experimental/worktree (worktree.remove)` | `packages/sdk/openapi.json:1366` | not-ported |
| BASELINE-HTTP-38bdaa8c7405 | http | `DELETE /mcp/{name}/auth (mcp.auth.remove)` | `packages/sdk/openapi.json:3208` | not-ported |
| BASELINE-HTTP-eae8a4dc09e7 | http | `DELETE /pty/{ptyID} (pty.remove)` | `packages/sdk/openapi.json:4419` | not-ported |
| BASELINE-HTTP-947f00b87a5c | http | `DELETE /session/{sessionID} (session.delete)` | `packages/sdk/openapi.json:5660` | unverified |
| BASELINE-HTTP-b92aa3a00746 | http | `DELETE /session/{sessionID}/message/{messageID} (session.deleteMessage)` | `packages/sdk/openapi.json:6469` | unverified |
| BASELINE-HTTP-a9e8e7bfa7dc | http | `DELETE /session/{sessionID}/message/{messageID}/part/{partID} (part.delete)` | `packages/sdk/openapi.json:7860` | not-ported |
| BASELINE-HTTP-bd62ff716340 | http | `DELETE /session/{sessionID}/share (session.unshare)` | `packages/sdk/openapi.json:6921` | not-ported |
| BASELINE-HTTP-d71c5121cb8d | http | `GET /agent (app.agents)` | `packages/sdk/openapi.json:2725` | not-ported |
| BASELINE-HTTP-5619c10d8c36 | http | `GET /api/agent (v2.agent.list)` | `packages/sdk/openapi.json:9909` | not-ported |
| BASELINE-HTTP-3e41ad322abe | http | `GET /api/command (v2.command.list)` | `packages/sdk/openapi.json:13608` | not-ported |
| BASELINE-HTTP-b221c1a06b55 | http | `GET /api/event (v2.event.subscribe)` | `packages/sdk/openapi.json:13770` | unverified |
| BASELINE-HTTP-7bc40e172285 | http | `GET /api/fs/find (v2.fs.find)` | `packages/sdk/openapi.json:13502` | not-ported |
| BASELINE-HTTP-a18110e42a38 | http | `GET /api/fs/list (v2.fs.list)` | `packages/sdk/openapi.json:13413` | not-ported |
| BASELINE-HTTP-ec12a433251b | http | `GET /api/fs/read/* (v2.fs.read)` | `packages/sdk/openapi.json:13344` | not-ported |
| BASELINE-HTTP-c21e221d2a0a | http | `GET /api/health (v2.health.get)` | `packages/sdk/openapi.json:9785` | unverified |
| BASELINE-HTTP-ec559a24f29f | http | `GET /api/integration (v2.integration.list)` | `packages/sdk/openapi.json:11954` | not-ported |
| BASELINE-HTTP-69c6bea95b84 | http | `GET /api/integration/attempt/{attemptID} (v2.integration.attempt.status)` | `packages/sdk/openapi.json:12336` | not-ported |
| BASELINE-HTTP-ea533a1fd558 | http | `GET /api/integration/{integrationID} (v2.integration.get)` | `packages/sdk/openapi.json:12035` | not-ported |
| BASELINE-HTTP-a9bf3d8e4b87 | http | `GET /api/location (v2.location.get)` | `packages/sdk/openapi.json:9841` | not-ported |
| BASELINE-HTTP-039394f48f76 | http | `GET /api/model (v2.model.list)` | `packages/sdk/openapi.json:11666` | not-ported |
| BASELINE-HTTP-b90580419b1f | http | `GET /api/permission/request (v2.permission.request.list)` | `packages/sdk/openapi.json:12734` | not-ported |
| BASELINE-HTTP-2cbb7c588012 | http | `GET /api/permission/saved (v2.permission.saved.list)` | `packages/sdk/openapi.json:12815` | not-ported |
| BASELINE-HTTP-f441fd12cbbf | http | `GET /api/provider (v2.provider.list)` | `packages/sdk/openapi.json:11757` | not-ported |
| BASELINE-HTTP-3d0e1e4d894d | http | `GET /api/provider/{providerID} (v2.provider.get)` | `packages/sdk/openapi.json:11848` | not-ported |
| BASELINE-HTTP-0c0cae82fb82 | http | `GET /api/pty (v2.pty.list)` | `packages/sdk/openapi.json:13818` | not-ported |
| BASELINE-HTTP-7881ce76e3e2 | http | `GET /api/pty/{ptyID} (v2.pty.get)` | `packages/sdk/openapi.json:14009` | not-ported |
| BASELINE-HTTP-fbffbf62858b | http | `GET /api/pty/{ptyID}/connect (v2.pty.connect)` | `packages/sdk/openapi.json:14417` | unverified |
| BASELINE-HTTP-bf811950e58e | http | `GET /api/question/request (v2.question.request.list)` | `packages/sdk/openapi.json:14524` | not-ported |
| BASELINE-HTTP-eaedc8f96ae9 | http | `GET /api/reference (v2.reference.list)` | `packages/sdk/openapi.json:14860` | not-ported |
| BASELINE-HTTP-0fe518418cd9 | http | `GET /api/session (v2.session.list)` | `packages/sdk/openapi.json:9990` | not-ported |
| BASELINE-HTTP-bc57c444eb09 | http | `GET /api/session/active (v2.session.active)` | `packages/sdk/openapi.json:10195` | not-ported |
| BASELINE-HTTP-2e986ba2b95a | http | `GET /api/session/{sessionID} (v2.session.get)` | `packages/sdk/openapi.json:10255` | not-ported |
| BASELINE-HTTP-8b9b2cae92aa | http | `GET /api/session/{sessionID}/context (v2.session.context)` | `packages/sdk/openapi.json:11043` | not-ported |
| BASELINE-HTTP-e74456439556 | http | `GET /api/session/{sessionID}/event (v2.session.events)` | `packages/sdk/openapi.json:11229` | unverified |
| BASELINE-HTTP-03c7710a3aef | http | `GET /api/session/{sessionID}/history (v2.session.history)` | `packages/sdk/openapi.json:11138` | not-ported |
| BASELINE-HTTP-100d873a2d05 | http | `GET /api/session/{sessionID}/message (v2.session.messages)` | `packages/sdk/openapi.json:11548` | not-ported |
| BASELINE-HTTP-83220ee5bd78 | http | `GET /api/session/{sessionID}/message/{messageID} (v2.session.message)` | `packages/sdk/openapi.json:11454` | not-ported |
| BASELINE-HTTP-39ad94816cf9 | http | `GET /api/session/{sessionID}/permission (v2.session.permission.list)` | `packages/sdk/openapi.json:13065` | not-ported |
| BASELINE-HTTP-8843789bf573 | http | `GET /api/session/{sessionID}/permission/{requestID} (v2.session.permission.get)` | `packages/sdk/openapi.json:13150` | not-ported |
| BASELINE-HTTP-32ec1adf9d7a | http | `GET /api/session/{sessionID}/question (v2.session.question.list)` | `packages/sdk/openapi.json:14605` | not-ported |
| BASELINE-HTTP-5956800d49a9 | http | `GET /api/skill (v2.skill.list)` | `packages/sdk/openapi.json:13689` | not-ported |
| BASELINE-HTTP-3aaa95301d80 | http | `GET /command (command.list)` | `packages/sdk/openapi.json:2667` | not-ported |
| BASELINE-HTTP-a9ff93884c1e | http | `GET /config (config.get)` | `packages/sdk/openapi.json:625` | not-ported |
| BASELINE-HTTP-29e73d051ba5 | http | `GET /config/providers (config.providers)` | `packages/sdk/openapi.json:747` | not-ported |
| BASELINE-HTTP-ba33567901f2 | http | `GET /event (event.subscribe)` | `packages/sdk/openapi.json:581` | unverified |
| BASELINE-HTTP-c7c257309ac9 | http | `GET /experimental/capabilities (experimental.capabilities.get)` | `packages/sdk/openapi.json:818` | not-ported |
| BASELINE-HTTP-03daeaf8aca2 | http | `GET /experimental/console (experimental.console.get)` | `packages/sdk/openapi.json:872` | not-ported |
| BASELINE-HTTP-4a18da17cae3 | http | `GET /experimental/console/orgs (experimental.console.listOrgs)` | `packages/sdk/openapi.json:936` | not-ported |
| BASELINE-HTTP-a56fd8b540ad | http | `GET /experimental/resource (experimental.resource.list)` | `packages/sdk/openapi.json:1701` | not-ported |
| BASELINE-HTTP-81151c92178d | http | `GET /experimental/session (experimental.session.list)` | `packages/sdk/openapi.json:1508` | not-ported |
| BASELINE-HTTP-7068b6f4dedf | http | `GET /experimental/tool (tool.list)` | `packages/sdk/openapi.json:1097` | not-ported |
| BASELINE-HTTP-0a7cec4aa3af | http | `GET /experimental/tool/ids (tool.ids)` | `packages/sdk/openapi.json:1174` | not-ported |
| BASELINE-HTTP-c3fecc2e4c81 | http | `GET /experimental/workspace (experimental.workspace.list)` | `packages/sdk/openapi.json:9354` | not-ported |
| BASELINE-HTTP-70d49bf482b1 | http | `GET /experimental/workspace/adapter (experimental.workspace.adapter.list)` | `packages/sdk/openapi.json:9283` | not-ported |
| BASELINE-HTTP-4f4be331cd00 | http | `GET /experimental/workspace/status (experimental.workspace.status)` | `packages/sdk/openapi.json:9559` | not-ported |
| BASELINE-HTTP-299607bf8fee | http | `GET /experimental/worktree (worktree.list)` | `packages/sdk/openapi.json:1235` | not-ported |
| BASELINE-HTTP-732f4315deed | http | `GET /file (file.list)` | `packages/sdk/openapi.json:2045` | not-ported |
| BASELINE-HTTP-39328e56756f | http | `GET /file/content (file.read)` | `packages/sdk/openapi.json:2111` | not-ported |
| BASELINE-HTTP-e2e5e1330597 | http | `GET /file/status (file.status)` | `packages/sdk/openapi.json:2173` | not-ported |
| BASELINE-HTTP-ad57a311352a | http | `GET /find (find.text)` | `packages/sdk/openapi.json:1759` | not-ported |
| BASELINE-HTTP-c42e214ba714 | http | `GET /find/file (find.files)` | `packages/sdk/openapi.json:1885` | not-ported |
| BASELINE-HTTP-9b9c316066a0 | http | `GET /find/symbol (find.symbols)` | `packages/sdk/openapi.json:1979` | not-ported |
| BASELINE-HTTP-8cb8aff188f4 | http | `GET /formatter (formatter.status)` | `packages/sdk/openapi.json:2915` | not-ported |
| BASELINE-HTTP-992cce0334f6 | http | `GET /global/config (global.config.get)` | `packages/sdk/openapi.json:365` | not-ported |
| BASELINE-HTTP-0026254966e2 | http | `GET /global/event (global.event)` | `packages/sdk/openapi.json:328` | unverified |
| BASELINE-HTTP-785525bed184 | http | `GET /global/health (global.health)` | `packages/sdk/openapi.json:279` | unverified |
| BASELINE-HTTP-a52ed26f2a8e | http | `GET /lsp (lsp.status)` | `packages/sdk/openapi.json:2857` | not-ported |
| BASELINE-HTTP-bde9c03092cd | http | `GET /mcp (mcp.status)` | `packages/sdk/openapi.json:2973` | not-ported |
| BASELINE-HTTP-6de44b865b66 | http | `GET /path (path.get)` | `packages/sdk/openapi.json:2286` | not-ported |
| BASELINE-HTTP-1de71ab4f4b5 | http | `GET /permission (permission.list)` | `packages/sdk/openapi.json:4816` | not-ported |
| BASELINE-HTTP-ce2d1f6a6c8b | http | `GET /project (project.list)` | `packages/sdk/openapi.json:3607` | not-ported |
| BASELINE-HTTP-194aefbfcd89 | http | `GET /project/current (project.current)` | `packages/sdk/openapi.json:3665` | not-ported |
| BASELINE-HTTP-dd3548243a9c | http | `GET /project/{projectID}/directories (project.directories)` | `packages/sdk/openapi.json:3873` | not-ported |
| BASELINE-HTTP-57628c8a02eb | http | `GET /provider (provider.list)` | `packages/sdk/openapi.json:4975` | not-ported |
| BASELINE-HTTP-7da8a80fdc23 | http | `GET /provider/auth (provider.auth)` | `packages/sdk/openapi.json:5052` | not-ported |
| BASELINE-HTTP-19bb06964607 | http | `GET /pty (pty.list)` | `packages/sdk/openapi.json:4090` | not-ported |
| BASELINE-HTTP-013827429717 | http | `GET /pty/shells (pty.shells)` | `packages/sdk/openapi.json:4019` | not-ported |
| BASELINE-HTTP-f4861d9ce713 | http | `GET /pty/{ptyID} (pty.get)` | `packages/sdk/openapi.json:4240` | not-ported |
| BASELINE-HTTP-0fd1b3dad9d5 | http | `GET /pty/{ptyID}/connect (pty.connect)` | `packages/sdk/openapi.json:15176` | unverified |
| BASELINE-HTTP-a72b1cb8ec01 | http | `GET /question (question.list)` | `packages/sdk/openapi.json:4576` | not-ported |
| BASELINE-HTTP-c6fb7db8cc6c | http | `GET /session (session.list)` | `packages/sdk/openapi.json:5295` | not-ported |
| BASELINE-HTTP-2876b1e4a63a | http | `GET /session/status (session.status)` | `packages/sdk/openapi.json:5517` | not-ported |
| BASELINE-HTTP-d5155534558c | http | `GET /session/{sessionID} (session.get)` | `packages/sdk/openapi.json:5582` | not-ported |
| BASELINE-HTTP-eac03f49c3f7 | http | `GET /session/{sessionID}/children (session.children)` | `packages/sdk/openapi.json:5849` | not-ported |
| BASELINE-HTTP-e096309509bd | http | `GET /session/{sessionID}/diff (session.diff)` | `packages/sdk/openapi.json:6017` | not-ported |
| BASELINE-HTTP-9aab948abfa6 | http | `GET /session/{sessionID}/message (session.messages)` | `packages/sdk/openapi.json:6093` | not-ported |
| BASELINE-HTTP-cc677d7c675c | http | `GET /session/{sessionID}/message/{messageID} (session.message)` | `packages/sdk/openapi.json:6368` | not-ported |
| BASELINE-HTTP-ffb898582941 | http | `GET /session/{sessionID}/todo (session.todo)` | `packages/sdk/openapi.json:5933` | not-ported |
| BASELINE-HTTP-4d8b2ff01871 | http | `GET /skill (app.skills)` | `packages/sdk/openapi.json:2783` | not-ported |
| BASELINE-HTTP-b74cccccecdc | http | `GET /tui/control/next (tui.control.next)` | `packages/sdk/openapi.json:9158` | unverified |
| BASELINE-HTTP-84df6713c5d1 | http | `GET /vcs (vcs.get)` | `packages/sdk/openapi.json:2340` | not-ported |
| BASELINE-HTTP-26b1cc0048e8 | http | `GET /vcs/diff (vcs.diff)` | `packages/sdk/openapi.json:2452` | not-ported |
| BASELINE-HTTP-7a7b2c63baf4 | http | `GET /vcs/diff/raw (vcs.diff.raw)` | `packages/sdk/openapi.json:2528` | not-ported |
| BASELINE-HTTP-aab92f8b8a1a | http | `GET /vcs/status (vcs.status)` | `packages/sdk/openapi.json:2394` | not-ported |
| BASELINE-HTTP-bc935353c299 | http | `PATCH /api/credential/{credentialID} (v2.credential.update)` | `packages/sdk/openapi.json:12581` | not-ported |
| BASELINE-HTTP-0a9e97d9b2f8 | http | `PATCH /config (config.update)` | `packages/sdk/openapi.json:677` | not-ported |
| BASELINE-HTTP-90424aec0f2a | http | `PATCH /global/config (global.config.update)` | `packages/sdk/openapi.json:400` | not-ported |
| BASELINE-HTTP-45f6b6147eaa | http | `PATCH /project/{projectID} (project.update)` | `packages/sdk/openapi.json:3773` | not-ported |
| BASELINE-HTTP-871dcfb23e08 | http | `PATCH /session/{sessionID} (session.update)` | `packages/sdk/openapi.json:5739` | unverified |
| BASELINE-HTTP-571ab208e8b2 | http | `PATCH /session/{sessionID}/message/{messageID}/part/{partID} (part.update)` | `packages/sdk/openapi.json:7956` | not-ported |
| BASELINE-HTTP-c584d98b33f8 | http | `POST /api/integration/attempt/{attemptID}/complete (v2.integration.attempt.complete)` | `packages/sdk/openapi.json:12489` | not-ported |
| BASELINE-HTTP-56564bf6b461 | http | `POST /api/integration/{integrationID}/connect/key (v2.integration.connect.key)` | `packages/sdk/openapi.json:12121` | not-ported |
| BASELINE-HTTP-e052c22a9890 | http | `POST /api/integration/{integrationID}/connect/oauth (v2.integration.connect.oauth)` | `packages/sdk/openapi.json:12217` | not-ported |
| BASELINE-HTTP-19fa3c0be7a7 | http | `POST /api/pty (v2.pty.create)` | `packages/sdk/openapi.json:13897` | unverified |
| BASELINE-HTTP-bdbb27278c30 | http | `POST /api/pty/{ptyID}/connect-token (v2.pty.connectToken)` | `packages/sdk/openapi.json:14310` | not-ported |
| BASELINE-HTTP-e2f3c9cebd06 | http | `POST /api/session (v2.session.create)` | `packages/sdk/openapi.json:10114` | not-ported |
| BASELINE-HTTP-dca1522d6ae2 | http | `POST /api/session/{sessionID}/agent (v2.session.switchAgent)` | `packages/sdk/openapi.json:10337` | not-ported |
| BASELINE-HTTP-6e3251dc3e35 | http | `POST /api/session/{sessionID}/compact (v2.session.compact)` | `packages/sdk/openapi.json:10627` | unverified |
| BASELINE-HTTP-35c4d7eba627 | http | `POST /api/session/{sessionID}/interrupt (v2.session.interrupt)` | `packages/sdk/openapi.json:11386` | not-ported |
| BASELINE-HTTP-9dd53549c81d | http | `POST /api/session/{sessionID}/model (v2.session.switchModel)` | `packages/sdk/openapi.json:10422` | not-ported |
| BASELINE-HTTP-1d6946e090da | http | `POST /api/session/{sessionID}/permission (v2.session.permission.create)` | `packages/sdk/openapi.json:12932` | not-ported |
| BASELINE-HTTP-9e7c96718aad | http | `POST /api/session/{sessionID}/permission/{requestID}/reply (v2.session.permission.reply)` | `packages/sdk/openapi.json:13244` | not-ported |
| BASELINE-HTTP-57f9e29a4391 | http | `POST /api/session/{sessionID}/prompt (v2.session.prompt)` | `packages/sdk/openapi.json:10507` | not-ported |
| BASELINE-HTTP-f37e2f287290 | http | `POST /api/session/{sessionID}/question/{requestID}/reject (v2.session.question.reject)` | `packages/sdk/openapi.json:14780` | not-ported |
| BASELINE-HTTP-381c037d3f2e | http | `POST /api/session/{sessionID}/question/{requestID}/reply (v2.session.question.reply)` | `packages/sdk/openapi.json:14690` | not-ported |
| BASELINE-HTTP-fb405356866a | http | `POST /api/session/{sessionID}/revert/clear (v2.session.revert.clear)` | `packages/sdk/openapi.json:10899` | not-ported |
| BASELINE-HTTP-232fcd814d22 | http | `POST /api/session/{sessionID}/revert/commit (v2.session.revert.commit)` | `packages/sdk/openapi.json:10976` | not-ported |
| BASELINE-HTTP-28eae3880a8f | http | `POST /api/session/{sessionID}/revert/stage (v2.session.revert.stage)` | `packages/sdk/openapi.json:10783` | not-ported |
| BASELINE-HTTP-c9f380b0665d | http | `POST /api/session/{sessionID}/wait (v2.session.wait)` | `packages/sdk/openapi.json:10705` | unverified |
| BASELINE-HTTP-e1f46289d48e | http | `POST /experimental/console/switch (experimental.console.switchOrg)` | `packages/sdk/openapi.json:1033` | not-ported |
| BASELINE-HTTP-49e1a47fa50f | http | `POST /experimental/control-plane/move-session (experimental.controlPlane.moveSession)` | `packages/sdk/openapi.json:219` | not-ported |
| BASELINE-HTTP-5045ee9f26e1 | http | `POST /experimental/project/{projectID}/copy (v2.projectCopy.create)` | `packages/sdk/openapi.json:14941` | unverified |
| BASELINE-HTTP-b521d4dd787f | http | `POST /experimental/project/{projectID}/copy/generate-name (experimental.projectCopy.generateName)` | `packages/sdk/openapi.json:3935` | not-ported |
| BASELINE-HTTP-529cb806e383 | http | `POST /experimental/project/{projectID}/copy/refresh (v2.projectCopy.refresh)` | `packages/sdk/openapi.json:15113` | not-ported |
| BASELINE-HTTP-c222a8ef814c | http | `POST /experimental/session/{sessionID}/background (experimental.session.background)` | `packages/sdk/openapi.json:1630` | not-ported |
| BASELINE-HTTP-32c2ef36cf02 | http | `POST /experimental/workspace (experimental.workspace.create)` | `packages/sdk/openapi.json:9410` | unverified |
| BASELINE-HTTP-9b9e48549a06 | http | `POST /experimental/workspace/sync-list (experimental.workspace.syncList)` | `packages/sdk/openapi.json:9512` | not-ported |
| BASELINE-HTTP-208f5d43d499 | http | `POST /experimental/workspace/warp (experimental.workspace.warp)` | `packages/sdk/openapi.json:9687` | not-ported |
| BASELINE-HTTP-e040adc21f2a | http | `POST /experimental/worktree (worktree.create)` | `packages/sdk/openapi.json:1298` | unverified |
| BASELINE-HTTP-aad82b3e875a | http | `POST /experimental/worktree/reset (worktree.reset)` | `packages/sdk/openapi.json:1437` | not-ported |
| BASELINE-HTTP-d89f5f00363a | http | `POST /global/dispose (global.dispose)` | `packages/sdk/openapi.json:453` | not-ported |
| BASELINE-HTTP-108519957bac | http | `POST /global/upgrade (global.upgrade)` | `packages/sdk/openapi.json:491` | unverified |
| BASELINE-HTTP-71d6c4dd56c8 | http | `POST /instance/dispose (instance.dispose)` | `packages/sdk/openapi.json:2231` | not-ported |
| BASELINE-HTTP-0e9abd7254b5 | http | `POST /log (app.log)` | `packages/sdk/openapi.json:128` | not-ported |
| BASELINE-HTTP-9c63a1e6ea81 | http | `POST /mcp (mcp.add)` | `packages/sdk/openapi.json:3029` | not-ported |
| BASELINE-HTTP-8e538ff159ef | http | `POST /mcp/{name}/auth (mcp.auth.start)` | `packages/sdk/openapi.json:3120` | not-ported |
| BASELINE-HTTP-c3e457fbecb9 | http | `POST /mcp/{name}/auth/authenticate (mcp.auth.authenticate)` | `packages/sdk/openapi.json:3384` | not-ported |
| BASELINE-HTTP-f0565f3d9c13 | http | `POST /mcp/{name}/auth/callback (mcp.auth.callback)` | `packages/sdk/openapi.json:3289` | not-ported |
| BASELINE-HTTP-0bb511c4ba3e | http | `POST /mcp/{name}/connect (mcp.connect)` | `packages/sdk/openapi.json:3463` | not-ported |
| BASELINE-HTTP-628cf6368ff4 | http | `POST /mcp/{name}/disconnect (mcp.disconnect)` | `packages/sdk/openapi.json:3535` | not-ported |
| BASELINE-HTTP-a2e6d12c219f | http | `POST /permission/{requestID}/reply (permission.reply)` | `packages/sdk/openapi.json:4874` | not-ported |
| BASELINE-HTTP-280d8907ba0a | http | `POST /project/git/init (project.initGit)` | `packages/sdk/openapi.json:3719` | not-ported |
| BASELINE-HTTP-ecf221f3811a | http | `POST /provider/{providerID}/oauth/authorize (provider.oauth.authorize)` | `packages/sdk/openapi.json:5113` | not-ported |
| BASELINE-HTTP-a4d3ec2cae65 | http | `POST /provider/{providerID}/oauth/callback (provider.oauth.callback)` | `packages/sdk/openapi.json:5205` | not-ported |
| BASELINE-HTTP-7865aab1ef68 | http | `POST /pty (pty.create)` | `packages/sdk/openapi.json:4146` | unverified |
| BASELINE-HTTP-d981c005270d | http | `POST /pty/{ptyID}/connect-token (pty.connectToken)` | `packages/sdk/openapi.json:4493` | not-ported |
| BASELINE-HTTP-ccafe0e2b859 | http | `POST /question/{requestID}/reject (question.reject)` | `packages/sdk/openapi.json:4735` | not-ported |
| BASELINE-HTTP-caa0e251e08c | http | `POST /question/{requestID}/reply (question.reply)` | `packages/sdk/openapi.json:4634` | not-ported |
| BASELINE-HTTP-da2ef7e1c88e | http | `POST /session (session.create)` | `packages/sdk/openapi.json:5408` | not-ported |
| BASELINE-HTTP-8c2f57a397ca | http | `POST /session/{sessionID}/abort (session.abort)` | `packages/sdk/openapi.json:6665` | not-ported |
| BASELINE-HTTP-c72a852d8e63 | http | `POST /session/{sessionID}/command (session.command)` | `packages/sdk/openapi.json:7250` | not-ported |
| BASELINE-HTTP-e19a9f2593ee | http | `POST /session/{sessionID}/fork (session.fork)` | `packages/sdk/openapi.json:6569` | not-ported |
| BASELINE-HTTP-21f6793512f0 | http | `POST /session/{sessionID}/init (session.init)` | `packages/sdk/openapi.json:6736` | not-ported |
| BASELINE-HTTP-0f99dc348648 | http | `POST /session/{sessionID}/message (session.prompt)` | `packages/sdk/openapi.json:6206` | unverified |
| BASELINE-HTTP-4061b18f3b9d | http | `POST /session/{sessionID}/permissions/{permissionID} (permission.respond)` | `packages/sdk/openapi.json:7745` | not-ported |
| BASELINE-HTTP-f41003a65a29 | http | `POST /session/{sessionID}/prompt_async (session.prompt_async)` | `packages/sdk/openapi.json:7107` | not-ported |
| BASELINE-HTTP-1d3accfb1a6d | http | `POST /session/{sessionID}/revert (session.revert)` | `packages/sdk/openapi.json:7544` | not-ported |
| BASELINE-HTTP-766b614a5d25 | http | `POST /session/{sessionID}/share (session.share)` | `packages/sdk/openapi.json:6840` | not-ported |
| BASELINE-HTTP-9b51707eeec4 | http | `POST /session/{sessionID}/shell (session.shell)` | `packages/sdk/openapi.json:7404` | unverified |
| BASELINE-HTTP-96cc86238391 | http | `POST /session/{sessionID}/summarize (session.summarize)` | `packages/sdk/openapi.json:7004` | not-ported |
| BASELINE-HTTP-8749637da9ae | http | `POST /session/{sessionID}/unrevert (session.unrevert)` | `packages/sdk/openapi.json:7655` | not-ported |
| BASELINE-HTTP-84aa9b961ad3 | http | `POST /sync/history (sync.history.list)` | `packages/sdk/openapi.json:8317` | not-ported |
| BASELINE-HTTP-4ed83551ddef | http | `POST /sync/replay (sync.replay)` | `packages/sdk/openapi.json:8117` | not-ported |
| BASELINE-HTTP-5187ef9ed88a | http | `POST /sync/start (sync.start)` | `packages/sdk/openapi.json:8062` | not-ported |
| BASELINE-HTTP-87a5d2dd1d7c | http | `POST /sync/steal (sync.steal)` | `packages/sdk/openapi.json:8230` | not-ported |
| BASELINE-HTTP-bf6b9a68a9a7 | http | `POST /tui/append-prompt (tui.appendPrompt)` | `packages/sdk/openapi.json:8416` | not-ported |
| BASELINE-HTTP-13298874584f | http | `POST /tui/clear-prompt (tui.clearPrompt)` | `packages/sdk/openapi.json:8769` | not-ported |
| BASELINE-HTTP-427932ef6389 | http | `POST /tui/control/response (tui.control.response)` | `packages/sdk/openapi.json:9221` | not-ported |
| BASELINE-HTTP-b3c60482ff9e | http | `POST /tui/execute-command (tui.executeCommand)` | `packages/sdk/openapi.json:8824` | not-ported |
| BASELINE-HTTP-52463f73e487 | http | `POST /tui/open-help (tui.openHelp)` | `packages/sdk/openapi.json:8494` | not-ported |
| BASELINE-HTTP-904c2e7ca061 | http | `POST /tui/open-models (tui.openModels)` | `packages/sdk/openapi.json:8659` | not-ported |
| BASELINE-HTTP-81057e2cfe32 | http | `POST /tui/open-sessions (tui.openSessions)` | `packages/sdk/openapi.json:8549` | not-ported |
| BASELINE-HTTP-6080b78135e1 | http | `POST /tui/open-themes (tui.openThemes)` | `packages/sdk/openapi.json:8604` | not-ported |
| BASELINE-HTTP-11987fe95007 | http | `POST /tui/publish (tui.publish)` | `packages/sdk/openapi.json:8984` | not-ported |
| BASELINE-HTTP-4c6f368201da | http | `POST /tui/select-session (tui.selectSession)` | `packages/sdk/openapi.json:9068` | not-ported |
| BASELINE-HTTP-4cbd485b3c9a | http | `POST /tui/show-toast (tui.showToast)` | `packages/sdk/openapi.json:8902` | not-ported |
| BASELINE-HTTP-cb298958b871 | http | `POST /tui/submit-prompt (tui.submitPrompt)` | `packages/sdk/openapi.json:8714` | not-ported |
| BASELINE-HTTP-450a2ed7b6a0 | http | `POST /vcs/apply (vcs.apply)` | `packages/sdk/openapi.json:2582` | not-ported |
| BASELINE-HTTP-6e042d378878 | http | `PUT /api/pty/{ptyID} (v2.pty.update)` | `packages/sdk/openapi.json:14104` | not-ported |
| BASELINE-HTTP-3a5766f7e9ea | http | `PUT /auth/{providerID} (auth.set)` | `packages/sdk/openapi.json:13` | not-ported |
| BASELINE-HTTP-5858e6349219 | http | `PUT /pty/{ptyID} (pty.update)` | `packages/sdk/openapi.json:4311` | not-ported |
| BASELINE-HTTP-DECLARATION-e6886d639ef8 | http-declaration | `abort` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:253` | unverified |
| BASELINE-HTTP-DECLARATION-030fd6a327dd | http-declaration | `adapters` | `packages/opencode/src/server/routes/instance/httpapi/groups/workspace.ts:53` | unverified |
| BASELINE-HTTP-DECLARATION-6f4264fe7792 | http-declaration | `add` | `packages/opencode/src/server/routes/instance/httpapi/groups/mcp.ts:55` | unverified |
| BASELINE-HTTP-DECLARATION-a302e27994b1 | http-declaration | `agent` | `packages/opencode/src/server/routes/instance/httpapi/groups/instance.ts:149` | unverified |
| BASELINE-HTTP-DECLARATION-fed50781fa50 | http-declaration | `agent.list` | `packages/protocol/src/groups/agent.ts:8` | unverified |
| BASELINE-HTTP-DECLARATION-4c94df255f6a | http-declaration | `appendPrompt` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:56` | unverified |
| BASELINE-HTTP-DECLARATION-2ad9d11ba6cb | http-declaration | `auth` | `packages/opencode/src/server/routes/instance/httpapi/groups/provider.ts:48` | unverified |
| BASELINE-HTTP-DECLARATION-62fe7b1cc994 | http-declaration | `authAuthenticate` | `packages/opencode/src/server/routes/instance/httpapi/groups/mcp.ts:93` | unverified |
| BASELINE-HTTP-DECLARATION-512d367aa3f1 | http-declaration | `authCallback` | `packages/opencode/src/server/routes/instance/httpapi/groups/mcp.ts:79` | unverified |
| BASELINE-HTTP-DECLARATION-20ae20074560 | http-declaration | `authRemove` | `packages/opencode/src/server/routes/instance/httpapi/groups/control.ts:51` | unverified |
| BASELINE-HTTP-DECLARATION-d3787502ef53 | http-declaration | `authRemove` | `packages/opencode/src/server/routes/instance/httpapi/groups/mcp.ts:105` | unverified |
| BASELINE-HTTP-DECLARATION-6a9ef2fc4f66 | http-declaration | `authSet` | `packages/opencode/src/server/routes/instance/httpapi/groups/control.ts:39` | unverified |
| BASELINE-HTTP-DECLARATION-379aee3cddb9 | http-declaration | `authStart` | `packages/opencode/src/server/routes/instance/httpapi/groups/mcp.ts:67` | unverified |
| BASELINE-HTTP-DECLARATION-42f0729e66d1 | http-declaration | `authorize` | `packages/opencode/src/server/routes/instance/httpapi/groups/provider.ts:58` | unverified |
| BASELINE-HTTP-DECLARATION-f0e619c6ffd8 | http-declaration | `callback` | `packages/opencode/src/server/routes/instance/httpapi/groups/provider.ts:71` | unverified |
| BASELINE-HTTP-DECLARATION-9fafb20ed790 | http-declaration | `capabilities` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:108` | unverified |
| BASELINE-HTTP-DECLARATION-1da3a613380a | http-declaration | `children` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:144` | unverified |
| BASELINE-HTTP-DECLARATION-6e3d51cc3a99 | http-declaration | `clearPrompt` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:118` | unverified |
| BASELINE-HTTP-DECLARATION-ca459b969281 | http-declaration | `command` | `packages/opencode/src/server/routes/instance/httpapi/groups/instance.ts:139` | unverified |
| BASELINE-HTTP-DECLARATION-8b50d4845b30 | http-declaration | `command` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:343` | unverified |
| BASELINE-HTTP-DECLARATION-2717c0019f5d | http-declaration | `command.list` | `packages/protocol/src/groups/command.ts:9` | unverified |
| BASELINE-HTTP-DECLARATION-24d7df2cef03 | http-declaration | `configGet` | `packages/opencode/src/server/routes/instance/httpapi/groups/global.ts:97` | unverified |
| BASELINE-HTTP-DECLARATION-fd7ac5c1ee73 | http-declaration | `configUpdate` | `packages/opencode/src/server/routes/instance/httpapi/groups/global.ts:106` | unverified |
| BASELINE-HTTP-DECLARATION-8db45adff3cd | http-declaration | `connect` | `packages/opencode/src/server/routes/instance/httpapi/groups/mcp.ts:117` | unverified |
| BASELINE-HTTP-DECLARATION-5bd817e14bd3 | http-declaration | `connect` | `packages/opencode/src/server/routes/instance/httpapi/groups/pty.ts:144` | unverified |
| BASELINE-HTTP-DECLARATION-5674bfb08c73 | http-declaration | `connectToken` | `packages/opencode/src/server/routes/instance/httpapi/groups/pty.ts:113` | unverified |
| BASELINE-HTTP-DECLARATION-9f2e8d29277d | http-declaration | `console` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:118` | unverified |
| BASELINE-HTTP-DECLARATION-7227830c2846 | http-declaration | `consoleOrgs` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:129` | unverified |
| BASELINE-HTTP-DECLARATION-13752bb0c546 | http-declaration | `consoleSwitch` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:140` | unverified |
| BASELINE-HTTP-DECLARATION-c60dd0484244 | http-declaration | `content` | `packages/opencode/src/server/routes/instance/httpapi/groups/file.ts:148` | unverified |
| BASELINE-HTTP-DECLARATION-3ed66f01456f | http-declaration | `controlNext` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:175` | unverified |
| BASELINE-HTTP-DECLARATION-79117a723978 | http-declaration | `controlResponse` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:185` | unverified |
| BASELINE-HTTP-DECLARATION-5b201cfe8d6d | http-declaration | `create` | `packages/opencode/src/server/routes/instance/httpapi/groups/pty.ts:64` | unverified |
| BASELINE-HTTP-DECLARATION-27398facd202 | http-declaration | `create` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:203` | unverified |
| BASELINE-HTTP-DECLARATION-568c47c601ba | http-declaration | `create` | `packages/opencode/src/server/routes/instance/httpapi/groups/workspace.ts:73` | unverified |
| BASELINE-HTTP-DECLARATION-9c2bf49bb567 | http-declaration | `credential.remove` | `packages/protocol/src/groups/credential.ts:24` | unverified |
| BASELINE-HTTP-DECLARATION-0045407bd4a7 | http-declaration | `credential.update` | `packages/protocol/src/groups/credential.ts:8` | unverified |
| BASELINE-HTTP-DECLARATION-0834022f4c7d | http-declaration | `current` | `packages/opencode/src/server/routes/instance/httpapi/groups/project.ts:32` | unverified |
| BASELINE-HTTP-DECLARATION-bfbd7f33085c | http-declaration | `deleteMessage` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:409` | unverified |
| BASELINE-HTTP-DECLARATION-7eb6d2836b83 | http-declaration | `deletePart` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:422` | unverified |
| BASELINE-HTTP-DECLARATION-17c54d6cf34a | http-declaration | `diff` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:168` | unverified |
| BASELINE-HTTP-DECLARATION-03a3822d0c79 | http-declaration | `directories` | `packages/opencode/src/server/routes/instance/httpapi/groups/project.ts:65` | unverified |
| BASELINE-HTTP-DECLARATION-ea338497b666 | http-declaration | `disconnect` | `packages/opencode/src/server/routes/instance/httpapi/groups/mcp.ts:128` | unverified |
| BASELINE-HTTP-DECLARATION-be9930b16911 | http-declaration | `dispose` | `packages/opencode/src/server/routes/instance/httpapi/groups/global.ts:117` | unverified |
| BASELINE-HTTP-DECLARATION-d078f12735ae | http-declaration | `dispose` | `packages/opencode/src/server/routes/instance/httpapi/groups/instance.ts:62` | unverified |
| BASELINE-HTTP-DECLARATION-05acfbce1d1f | http-declaration | `event` | `packages/opencode/src/server/routes/instance/httpapi/groups/global.ts:88` | unverified |
| BASELINE-HTTP-DECLARATION-9c0cf16f88b0 | http-declaration | `event.subscribe` | `packages/protocol/src/groups/event.ts:35` | unverified |
| BASELINE-HTTP-DECLARATION-bbb9a92f46dd | http-declaration | `executeCommand` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:128` | unverified |
| BASELINE-HTTP-DECLARATION-ccb144400a89 | http-declaration | `findFile` | `packages/opencode/src/server/routes/instance/httpapi/groups/file.ts:118` | unverified |
| BASELINE-HTTP-DECLARATION-a2f969b4576f | http-declaration | `findSymbol` | `packages/opencode/src/server/routes/instance/httpapi/groups/file.ts:128` | unverified |
| BASELINE-HTTP-DECLARATION-a27bac576822 | http-declaration | `findText` | `packages/opencode/src/server/routes/instance/httpapi/groups/file.ts:108` | unverified |
| BASELINE-HTTP-DECLARATION-e7d882100986 | http-declaration | `fork` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:240` | unverified |
| BASELINE-HTTP-DECLARATION-3d819d76a959 | http-declaration | `formatter` | `packages/opencode/src/server/routes/instance/httpapi/groups/instance.ts:179` | unverified |
| BASELINE-HTTP-DECLARATION-e3a7ff6d1180 | http-declaration | `fs.find` | `packages/protocol/src/groups/fs.ts:50` | unverified |
| BASELINE-HTTP-DECLARATION-77a3698bd836 | http-declaration | `fs.list` | `packages/protocol/src/groups/fs.ts:36` | unverified |
| BASELINE-HTTP-DECLARATION-10d95c32d7b9 | http-declaration | `fs.read` | `packages/protocol/src/groups/fs.ts:22` | unverified |
| BASELINE-HTTP-DECLARATION-f2754734c4e4 | http-declaration | `generateName` | `packages/opencode/src/server/routes/instance/httpapi/groups/project-copy.ts:15` | unverified |
| BASELINE-HTTP-DECLARATION-8b5c911d6635 | http-declaration | `get` | `packages/opencode/src/server/routes/instance/httpapi/groups/config.ts:16` | unverified |
| BASELINE-HTTP-DECLARATION-3805a1a462e8 | http-declaration | `get` | `packages/opencode/src/server/routes/instance/httpapi/groups/pty.ts:76` | unverified |
| BASELINE-HTTP-DECLARATION-fa3776c8cbf3 | http-declaration | `get` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:132` | unverified |
| BASELINE-HTTP-DECLARATION-34a951ae492b | http-declaration | `health` | `packages/opencode/src/server/routes/instance/httpapi/groups/global.ts:79` | unverified |
| BASELINE-HTTP-DECLARATION-27c8b4dc42bb | http-declaration | `health.get` | `packages/protocol/src/groups/health.ts:5` | unverified |
| BASELINE-HTTP-DECLARATION-ee382c3f782d | http-declaration | `history` | `packages/opencode/src/server/routes/instance/httpapi/groups/sync.ts:83` | unverified |
| BASELINE-HTTP-DECLARATION-2ae4489760e3 | http-declaration | `init` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:265` | unverified |
| BASELINE-HTTP-DECLARATION-3f1c26ca28d1 | http-declaration | `initGit` | `packages/opencode/src/server/routes/instance/httpapi/groups/project.ts:42` | unverified |
| BASELINE-HTTP-DECLARATION-356899d35e61 | http-declaration | `integration.attempt.cancel` | `packages/protocol/src/groups/integration.ts:114` | unverified |
| BASELINE-HTTP-DECLARATION-bc5e4aa79a73 | http-declaration | `integration.attempt.complete` | `packages/protocol/src/groups/integration.ts:97` | unverified |
| BASELINE-HTTP-DECLARATION-b21bdb4cb3ab | http-declaration | `integration.attempt.status` | `packages/protocol/src/groups/integration.ts:82` | unverified |
| BASELINE-HTTP-DECLARATION-4b876cd4b7db | http-declaration | `integration.connect.key` | `packages/protocol/src/groups/integration.ts:41` | unverified |
| BASELINE-HTTP-DECLARATION-cd8f849adf2f | http-declaration | `integration.connect.oauth` | `packages/protocol/src/groups/integration.ts:61` | unverified |
| BASELINE-HTTP-DECLARATION-9864dcd0c525 | http-declaration | `integration.get` | `packages/protocol/src/groups/integration.ts:26` | unverified |
| BASELINE-HTTP-DECLARATION-77cb8cf4f720 | http-declaration | `integration.list` | `packages/protocol/src/groups/integration.ts:12` | unverified |
| BASELINE-HTTP-DECLARATION-846c0897a17d | http-declaration | `list` | `packages/opencode/src/server/routes/instance/httpapi/groups/file.ts:138` | unverified |
| BASELINE-HTTP-DECLARATION-bcbcc241b0e7 | http-declaration | `list` | `packages/opencode/src/server/routes/instance/httpapi/groups/permission.ts:21` | unverified |
| BASELINE-HTTP-DECLARATION-c3d5e07a2683 | http-declaration | `list` | `packages/opencode/src/server/routes/instance/httpapi/groups/project.ts:22` | unverified |
| BASELINE-HTTP-DECLARATION-9ff1c54f7279 | http-declaration | `list` | `packages/opencode/src/server/routes/instance/httpapi/groups/provider.ts:38` | unverified |
| BASELINE-HTTP-DECLARATION-b01b4bf3b8a6 | http-declaration | `list` | `packages/opencode/src/server/routes/instance/httpapi/groups/pty.ts:54` | unverified |
| BASELINE-HTTP-DECLARATION-b8ea16db2fc7 | http-declaration | `list` | `packages/opencode/src/server/routes/instance/httpapi/groups/question.ts:22` | unverified |
| BASELINE-HTTP-DECLARATION-3ebdef71e4a8 | http-declaration | `list` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:111` | unverified |
| BASELINE-HTTP-DECLARATION-ac8d989a47a1 | http-declaration | `list` | `packages/opencode/src/server/routes/instance/httpapi/groups/workspace.ts:63` | unverified |
| BASELINE-HTTP-DECLARATION-a53ad3bfb3b5 | http-declaration | `location.get` | `packages/protocol/src/groups/location.ts:30` | unverified |
| BASELINE-HTTP-DECLARATION-073c551a2903 | http-declaration | `log` | `packages/opencode/src/server/routes/instance/httpapi/groups/control.ts:62` | unverified |
| BASELINE-HTTP-DECLARATION-b9a28373c27d | http-declaration | `lsp` | `packages/opencode/src/server/routes/instance/httpapi/groups/instance.ts:169` | unverified |
| BASELINE-HTTP-DECLARATION-1c66ac8e9c16 | http-declaration | `message` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:191` | unverified |
| BASELINE-HTTP-DECLARATION-5c068c63a848 | http-declaration | `messages` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:179` | unverified |
| BASELINE-HTTP-DECLARATION-a3198013a7ca | http-declaration | `model.list` | `packages/protocol/src/groups/model.ts:10` | unverified |
| BASELINE-HTTP-DECLARATION-e8fcca145fd3 | http-declaration | `moveSession` | `packages/opencode/src/server/routes/instance/httpapi/groups/control-plane.ts:22` | unverified |
| BASELINE-HTTP-DECLARATION-c14f5cd0d991 | http-declaration | `openHelp` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:68` | unverified |
| BASELINE-HTTP-DECLARATION-d07de7a8e43b | http-declaration | `openModels` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:98` | unverified |
| BASELINE-HTTP-DECLARATION-ad90e656c9be | http-declaration | `openSessions` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:78` | unverified |
| BASELINE-HTTP-DECLARATION-623f9c4f2b3c | http-declaration | `openThemes` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:88` | unverified |
| BASELINE-HTTP-DECLARATION-feb2e6e6109d | http-declaration | `path` | `packages/opencode/src/server/routes/instance/httpapi/groups/instance.ts:72` | unverified |
| BASELINE-HTTP-DECLARATION-70f60d5a66cb | http-declaration | `permission.request.list` | `packages/protocol/src/groups/permission.ts:23` | unverified |
| BASELINE-HTTP-DECLARATION-60a0172613ac | http-declaration | `permission.saved.list` | `packages/protocol/src/groups/permission.ts:37` | unverified |
| BASELINE-HTTP-DECLARATION-fc7659ef50f2 | http-declaration | `permission.saved.remove` | `packages/protocol/src/groups/permission.ts:49` | unverified |
| BASELINE-HTTP-DECLARATION-c3c1b2ae8aa1 | http-declaration | `permissionRespond` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:395` | unverified |
| BASELINE-HTTP-DECLARATION-8910644e2f4b | http-declaration | `projectCopy.create` | `packages/protocol/src/groups/project-copy.ts:25` | unverified |
| BASELINE-HTTP-DECLARATION-078638fafe92 | http-declaration | `projectCopy.refresh` | `packages/protocol/src/groups/project-copy.ts:47` | unverified |
| BASELINE-HTTP-DECLARATION-2e1a7942ba6a | http-declaration | `projectCopy.remove` | `packages/protocol/src/groups/project-copy.ts:36` | unverified |
| BASELINE-HTTP-DECLARATION-0d60b4ee6475 | http-declaration | `prompt` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:316` | unverified |
| BASELINE-HTTP-DECLARATION-39bb75726e37 | http-declaration | `promptAsync` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:329` | unverified |
| BASELINE-HTTP-DECLARATION-b9f0fd99d968 | http-declaration | `provider.get` | `packages/protocol/src/groups/provider.ts:25` | unverified |
| BASELINE-HTTP-DECLARATION-d9b5736aac4e | http-declaration | `provider.list` | `packages/protocol/src/groups/provider.ts:10` | unverified |
| BASELINE-HTTP-DECLARATION-ffa0308f7cc9 | http-declaration | `providers` | `packages/opencode/src/server/routes/instance/httpapi/groups/config.ts:38` | unverified |
| BASELINE-HTTP-DECLARATION-40673b681395 | http-declaration | `pty.connect` | `packages/protocol/src/groups/pty.ts:119` | unverified |
| BASELINE-HTTP-DECLARATION-f6f92dcd7ba6 | http-declaration | `pty.connectToken` | `packages/protocol/src/groups/pty.ts:101` | unverified |
| BASELINE-HTTP-DECLARATION-9044bdf0cf1a | http-declaration | `pty.create` | `packages/protocol/src/groups/pty.ts:37` | unverified |
| BASELINE-HTTP-DECLARATION-2d7634819626 | http-declaration | `pty.get` | `packages/protocol/src/groups/pty.ts:52` | unverified |
| BASELINE-HTTP-DECLARATION-f0a3efd62414 | http-declaration | `pty.list` | `packages/protocol/src/groups/pty.ts:23` | unverified |
| BASELINE-HTTP-DECLARATION-25dbd358fa30 | http-declaration | `pty.remove` | `packages/protocol/src/groups/pty.ts:85` | unverified |
| BASELINE-HTTP-DECLARATION-454fe648fb35 | http-declaration | `pty.update` | `packages/protocol/src/groups/pty.ts:68` | unverified |
| BASELINE-HTTP-DECLARATION-3fddaf5a2987 | http-declaration | `publish` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:151` | unverified |
| BASELINE-HTTP-DECLARATION-4ac4d79e09d4 | http-declaration | `question.request.list` | `packages/protocol/src/groups/question.ts:20` | unverified |
| BASELINE-HTTP-DECLARATION-282886e3fdb2 | http-declaration | `reference.list` | `packages/protocol/src/groups/reference.ts:9` | unverified |
| BASELINE-HTTP-DECLARATION-d3433e855017 | http-declaration | `reject` | `packages/opencode/src/server/routes/instance/httpapi/groups/question.ts:45` | unverified |
| BASELINE-HTTP-DECLARATION-f3313bfb6182 | http-declaration | `remove` | `packages/opencode/src/server/routes/instance/httpapi/groups/pty.ts:101` | unverified |
| BASELINE-HTTP-DECLARATION-09ec3fae74a7 | http-declaration | `remove` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:215` | unverified |
| BASELINE-HTTP-DECLARATION-6a5367ed83b2 | http-declaration | `remove` | `packages/opencode/src/server/routes/instance/httpapi/groups/workspace.ts:105` | unverified |
| BASELINE-HTTP-DECLARATION-e13804258520 | http-declaration | `replay` | `packages/opencode/src/server/routes/instance/httpapi/groups/sync.ts:59` | unverified |
| BASELINE-HTTP-DECLARATION-f8c82efd4061 | http-declaration | `reply` | `packages/opencode/src/server/routes/instance/httpapi/groups/permission.ts:31` | unverified |
| BASELINE-HTTP-DECLARATION-b0bbcbd902dc | http-declaration | `reply` | `packages/opencode/src/server/routes/instance/httpapi/groups/question.ts:32` | unverified |
| BASELINE-HTTP-DECLARATION-9c07d1483184 | http-declaration | `resource` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:248` | unverified |
| BASELINE-HTTP-DECLARATION-06bc0c41abd6 | http-declaration | `revert` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:369` | unverified |
| BASELINE-HTTP-DECLARATION-7bd0966cee17 | http-declaration | `selectSession` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:163` | unverified |
| BASELINE-HTTP-DECLARATION-b842d8faad44 | http-declaration | `session` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:224` | unverified |
| BASELINE-HTTP-DECLARATION-295f89c17b61 | http-declaration | `session.active` | `packages/protocol/src/groups/session.ts:146` | unverified |
| BASELINE-HTTP-DECLARATION-f214d2a6c0ce | http-declaration | `session.compact` | `packages/protocol/src/groups/session.ts:226` | unverified |
| BASELINE-HTTP-DECLARATION-edac76bf01bf | http-declaration | `session.context` | `packages/protocol/src/groups/session.ts:292` | unverified |
| BASELINE-HTTP-DECLARATION-1512fb72fde5 | http-declaration | `session.create` | `packages/protocol/src/groups/session.ts:129` | unverified |
| BASELINE-HTTP-DECLARATION-6312f029898f | http-declaration | `session.events` | `packages/protocol/src/groups/session.ts:327` | unverified |
| BASELINE-HTTP-DECLARATION-a2f2b3206e7d | http-declaration | `session.get` | `packages/protocol/src/groups/session.ts:158` | unverified |
| BASELINE-HTTP-DECLARATION-2a88f7816655 | http-declaration | `session.history` | `packages/protocol/src/groups/session.ts:307` | unverified |
| BASELINE-HTTP-DECLARATION-1830cf559f68 | http-declaration | `session.interrupt` | `packages/protocol/src/groups/session.ts:345` | unverified |
| BASELINE-HTTP-DECLARATION-3f1a6c7949ec | http-declaration | `session.list` | `packages/protocol/src/groups/session.ts:109` | unverified |
| BASELINE-HTTP-DECLARATION-1eecb902a27a | http-declaration | `session.message` | `packages/protocol/src/groups/session.ts:360` | unverified |
| BASELINE-HTTP-DECLARATION-108fcb00b7c9 | http-declaration | `session.messages` | `packages/protocol/src/groups/message.ts:26` | unverified |
| BASELINE-HTTP-DECLARATION-97c220094f8d | http-declaration | `session.permission.create` | `packages/protocol/src/groups/permission.ts:63` | unverified |
| BASELINE-HTTP-DECLARATION-e6c0d70d4387 | http-declaration | `session.permission.get` | `packages/protocol/src/groups/permission.ts:104` | unverified |
| BASELINE-HTTP-DECLARATION-d0633ff02ac3 | http-declaration | `session.permission.list` | `packages/protocol/src/groups/permission.ts:89` | unverified |
| BASELINE-HTTP-DECLARATION-6e50759253f2 | http-declaration | `session.permission.reply` | `packages/protocol/src/groups/permission.ts:119` | unverified |
| BASELINE-HTTP-DECLARATION-75b137b50b67 | http-declaration | `session.prompt` | `packages/protocol/src/groups/session.ts:205` | unverified |
| BASELINE-HTTP-DECLARATION-f68532643d70 | http-declaration | `session.question.list` | `packages/protocol/src/groups/question.ts:37` | unverified |
| BASELINE-HTTP-DECLARATION-d45ed3b224d8 | http-declaration | `session.question.reject` | `packages/protocol/src/groups/question.ts:68` | unverified |
| BASELINE-HTTP-DECLARATION-1eb7711b46ea | http-declaration | `session.question.reply` | `packages/protocol/src/groups/question.ts:52` | unverified |
| BASELINE-HTTP-DECLARATION-b79527520a54 | http-declaration | `session.revert.clear` | `packages/protocol/src/groups/session.ts:272` | unverified |
| BASELINE-HTTP-DECLARATION-6773bbef2d34 | http-declaration | `session.revert.commit` | `packages/protocol/src/groups/session.ts:281` | unverified |
| BASELINE-HTTP-DECLARATION-2b99b876fd78 | http-declaration | `session.revert.stage` | `packages/protocol/src/groups/session.ts:256` | unverified |
| BASELINE-HTTP-DECLARATION-0ba4a59909ca | http-declaration | `session.switchAgent` | `packages/protocol/src/groups/session.ts:173` | unverified |
| BASELINE-HTTP-DECLARATION-cf7a280e67e2 | http-declaration | `session.switchModel` | `packages/protocol/src/groups/session.ts:189` | unverified |
| BASELINE-HTTP-DECLARATION-b59dd5fc7e1f | http-declaration | `session.wait` | `packages/protocol/src/groups/session.ts:241` | unverified |
| BASELINE-HTTP-DECLARATION-7881d28c6d8b | http-declaration | `sessionBackground` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:235` | unverified |
| BASELINE-HTTP-DECLARATION-95c8a2ca36f0 | http-declaration | `share` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:279` | unverified |
| BASELINE-HTTP-DECLARATION-6aa0bc524b0e | http-declaration | `shell` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:356` | unverified |
| BASELINE-HTTP-DECLARATION-479cdf7e5357 | http-declaration | `shells` | `packages/opencode/src/server/routes/instance/httpapi/groups/pty.ts:44` | unverified |
| BASELINE-HTTP-DECLARATION-608ae2868c38 | http-declaration | `showToast` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:140` | unverified |
| BASELINE-HTTP-DECLARATION-cf6f597e6abf | http-declaration | `skill` | `packages/opencode/src/server/routes/instance/httpapi/groups/instance.ts:159` | unverified |
| BASELINE-HTTP-DECLARATION-32c45ce0b2e5 | http-declaration | `skill.list` | `packages/protocol/src/groups/skill.ts:9` | unverified |
| BASELINE-HTTP-DECLARATION-46305ccd83a6 | http-declaration | `start` | `packages/opencode/src/server/routes/instance/httpapi/groups/sync.ts:49` | unverified |
| BASELINE-HTTP-DECLARATION-bba8c1eb52e5 | http-declaration | `status` | `packages/opencode/src/server/routes/instance/httpapi/groups/file.ts:158` | unverified |
| BASELINE-HTTP-DECLARATION-a094b27436b4 | http-declaration | `status` | `packages/opencode/src/server/routes/instance/httpapi/groups/mcp.ts:45` | unverified |
| BASELINE-HTTP-DECLARATION-3cf9732ede05 | http-declaration | `status` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:121` | unverified |
| BASELINE-HTTP-DECLARATION-c2a8a9875bf5 | http-declaration | `status` | `packages/opencode/src/server/routes/instance/httpapi/groups/workspace.ts:95` | unverified |
| BASELINE-HTTP-DECLARATION-117f8fbfbded | http-declaration | `steal` | `packages/opencode/src/server/routes/instance/httpapi/groups/sync.ts:71` | unverified |
| BASELINE-HTTP-DECLARATION-66cb20c1a37f | http-declaration | `submitPrompt` | `packages/opencode/src/server/routes/instance/httpapi/groups/tui.ts:108` | unverified |
| BASELINE-HTTP-DECLARATION-66f69234c7a8 | http-declaration | `subscribe` | `packages/opencode/src/server/routes/instance/httpapi/groups/event.ts:14` | unverified |
| BASELINE-HTTP-DECLARATION-d6f22aa7fc07 | http-declaration | `summarize` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:303` | unverified |
| BASELINE-HTTP-DECLARATION-6742b3ae7177 | http-declaration | `syncList` | `packages/opencode/src/server/routes/instance/httpapi/groups/workspace.ts:85` | unverified |
| BASELINE-HTTP-DECLARATION-b97b9f4dac55 | http-declaration | `todo` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:156` | unverified |
| BASELINE-HTTP-DECLARATION-e8e4002bf707 | http-declaration | `tool` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:152` | unverified |
| BASELINE-HTTP-DECLARATION-3989249901cf | http-declaration | `toolIDs` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:164` | unverified |
| BASELINE-HTTP-DECLARATION-435f8b1ed45c | http-declaration | `unrevert` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:383` | unverified |
| BASELINE-HTTP-DECLARATION-7859d072f3d8 | http-declaration | `unshare` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:291` | unverified |
| BASELINE-HTTP-DECLARATION-4d00185a5c4e | http-declaration | `update` | `packages/opencode/src/server/routes/instance/httpapi/groups/config.ts:26` | unverified |
| BASELINE-HTTP-DECLARATION-ed6eb3689d4c | http-declaration | `update` | `packages/opencode/src/server/routes/instance/httpapi/groups/project.ts:52` | unverified |
| BASELINE-HTTP-DECLARATION-302a75374ea5 | http-declaration | `update` | `packages/opencode/src/server/routes/instance/httpapi/groups/pty.ts:88` | unverified |
| BASELINE-HTTP-DECLARATION-df74e8adb39c | http-declaration | `update` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:227` | unverified |
| BASELINE-HTTP-DECLARATION-a3bf5a797e8a | http-declaration | `updatePart` | `packages/opencode/src/server/routes/instance/httpapi/groups/session.ts:433` | unverified |
| BASELINE-HTTP-DECLARATION-19688f055328 | http-declaration | `upgrade` | `packages/opencode/src/server/routes/instance/httpapi/groups/global.ts:126` | unverified |
| BASELINE-HTTP-DECLARATION-6627a3a658fe | http-declaration | `vcs` | `packages/opencode/src/server/routes/instance/httpapi/groups/instance.ts:83` | unverified |
| BASELINE-HTTP-DECLARATION-6b2ff967868f | http-declaration | `vcsApply` | `packages/opencode/src/server/routes/instance/httpapi/groups/instance.ts:127` | unverified |
| BASELINE-HTTP-DECLARATION-d7cf87dc91ed | http-declaration | `vcsDiff` | `packages/opencode/src/server/routes/instance/httpapi/groups/instance.ts:104` | unverified |
| BASELINE-HTTP-DECLARATION-14dffb6a5d3c | http-declaration | `vcsDiffRaw` | `packages/opencode/src/server/routes/instance/httpapi/groups/instance.ts:114` | unverified |
| BASELINE-HTTP-DECLARATION-292ad1273459 | http-declaration | `vcsStatus` | `packages/opencode/src/server/routes/instance/httpapi/groups/instance.ts:94` | unverified |
| BASELINE-HTTP-DECLARATION-f509614182f1 | http-declaration | `warp` | `packages/opencode/src/server/routes/instance/httpapi/groups/workspace.ts:117` | unverified |
| BASELINE-HTTP-DECLARATION-e438836e79ca | http-declaration | `worktree` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:176` | unverified |
| BASELINE-HTTP-DECLARATION-dd353cd34473 | http-declaration | `worktreeCreate` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:187` | unverified |
| BASELINE-HTTP-DECLARATION-ad8a302bd5a3 | http-declaration | `worktreeRemove` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:200` | unverified |
| BASELINE-HTTP-DECLARATION-bd6b1214d5d1 | http-declaration | `worktreeReset` | `packages/opencode/src/server/routes/instance/httpapi/groups/experimental.ts:212` | unverified |
| BASELINE-PROVIDER-35dea5690274 | provider | `@ai-sdk/alibaba` | `packages/opencode/src/provider/provider.ts:170` | unverified |
| BASELINE-PROVIDER-986954c23997 | provider | `@ai-sdk/amazon-bedrock` | `packages/opencode/src/provider/provider.ts:149` | unverified |
| BASELINE-PROVIDER-7e0ee3476d5d | provider | `@ai-sdk/amazon-bedrock/mantle` | `packages/opencode/src/provider/provider.ts:150` | unverified |
| BASELINE-PROVIDER-6f43c30ff51f | provider | `@ai-sdk/anthropic` | `packages/opencode/src/provider/provider.ts:151` | unverified |
| BASELINE-PROVIDER-f2b2246de836 | provider | `@ai-sdk/azure` | `packages/opencode/src/provider/provider.ts:152` | unverified |
| BASELINE-PROVIDER-9056f7577759 | provider | `@ai-sdk/cerebras` | `packages/opencode/src/provider/provider.ts:164` | unverified |
| BASELINE-PROVIDER-1e6428be5fd0 | provider | `@ai-sdk/cohere` | `packages/opencode/src/provider/provider.ts:165` | unverified |
| BASELINE-PROVIDER-03d198f63ecf | provider | `@ai-sdk/deepinfra` | `packages/opencode/src/provider/provider.ts:163` | unverified |
| BASELINE-PROVIDER-a0cb3d74f565 | provider | `@ai-sdk/gateway` | `packages/opencode/src/provider/provider.ts:166` | unverified |
| BASELINE-PROVIDER-a961938507b7 | provider | `@ai-sdk/github-copilot` | `packages/opencode/src/provider/provider.ts:172` | unverified |
| BASELINE-PROVIDER-e8686df1a490 | provider | `@ai-sdk/google` | `packages/opencode/src/provider/provider.ts:153` | unverified |
| BASELINE-PROVIDER-26db873702ed | provider | `@ai-sdk/google-vertex` | `packages/opencode/src/provider/provider.ts:154` | unverified |
| BASELINE-PROVIDER-7b4356133e0e | provider | `@ai-sdk/google-vertex/anthropic` | `packages/opencode/src/provider/provider.ts:155` | unverified |
| BASELINE-PROVIDER-68c996daf911 | provider | `@ai-sdk/groq` | `packages/opencode/src/provider/provider.ts:162` | unverified |
| BASELINE-PROVIDER-c09966ed0122 | provider | `@ai-sdk/mistral` | `packages/opencode/src/provider/provider.ts:161` | unverified |
| BASELINE-PROVIDER-cfc154e73eeb | provider | `@ai-sdk/openai` | `packages/opencode/src/provider/provider.ts:157` | unverified |
| BASELINE-PROVIDER-8dca5be1d19a | provider | `@ai-sdk/openai-compatible` | `packages/opencode/src/provider/provider.ts:158` | unverified |
| BASELINE-PROVIDER-51d3a8666638 | provider | `@ai-sdk/perplexity` | `packages/opencode/src/provider/provider.ts:168` | unverified |
| BASELINE-PROVIDER-052998bb57da | provider | `@ai-sdk/togetherai` | `packages/opencode/src/provider/provider.ts:167` | unverified |
| BASELINE-PROVIDER-637e8bf8fa6d | provider | `@ai-sdk/vercel` | `packages/opencode/src/provider/provider.ts:169` | unverified |
| BASELINE-PROVIDER-721014fc8d25 | provider | `@ai-sdk/xai` | `packages/opencode/src/provider/provider.ts:160` | unverified |
| BASELINE-PROVIDER-b0bfe200c2f7 | provider | `@openrouter/ai-sdk-provider` | `packages/opencode/src/provider/provider.ts:159` | unverified |
| BASELINE-PROVIDER-20ed74a6f554 | provider | `gitlab-ai-provider` | `packages/opencode/src/provider/provider.ts:171` | unverified |
| BASELINE-PROVIDER-72da16f364ac | provider | `venice-ai-sdk-provider` | `packages/opencode/src/provider/provider.ts:174` | unverified |
| BASELINE-TOOL-f482167e74cf | tool | `apply_patch` | `packages/core/src/tool/apply-patch.ts:17` | unverified |
| BASELINE-TOOL-c65c30045e75 | tool | `apply_patch` | `packages/opencode/src/tool/apply_patch.ts:23` | unverified |
| BASELINE-TOOL-06bd32055d27 | tool | `bash` | `packages/core/src/tool/bash.ts:18` | unverified |
| BASELINE-TOOL-b8a3160a3b6c | tool | `edit` | `packages/core/src/tool/edit.ts:22` | unverified |
| BASELINE-TOOL-b4297227e30c | tool | `edit` | `packages/opencode/src/tool/edit.ts:59` | unverified |
| BASELINE-TOOL-99e398e76f66 | tool | `glob` | `packages/core/src/tool/glob.ts:16` | unverified |
| BASELINE-TOOL-0282bb1743f3 | tool | `glob` | `packages/opencode/src/tool/glob.ts:18` | unverified |
| BASELINE-TOOL-7f0829c8ac4d | tool | `grep` | `packages/core/src/tool/grep.ts:17` | unverified |
| BASELINE-TOOL-32913056c76c | tool | `grep` | `packages/opencode/src/tool/grep.ts:21` | unverified |
| BASELINE-TOOL-72eacbca2906 | tool | `invalid` | `packages/opencode/src/tool/invalid.ts:10` | unverified |
| BASELINE-TOOL-3a8d540d3bfd | tool | `lsp` | `packages/opencode/src/tool/lsp.ts:38` | unverified |
| BASELINE-TOOL-1559ffd86088 | tool | `plan_exit` | `packages/opencode/src/tool/plan.ts:16` | unverified |
| BASELINE-TOOL-471f74a5ed68 | tool | `question` | `packages/core/src/tool/question.ts:12` | unverified |
| BASELINE-TOOL-03ad88d6261c | tool | `question` | `packages/opencode/src/tool/question.ts:15` | unverified |
| BASELINE-TOOL-f36822fb0968 | tool | `read` | `packages/core/src/tool/read.ts:16` | unverified |
| BASELINE-TOOL-cd7cf1b5c19d | tool | `read` | `packages/opencode/src/tool/read.ts:69` | unverified |
| BASELINE-TOOL-3743325578e1 | tool | `skill` | `packages/core/src/tool/skill.ts:14` | unverified |
| BASELINE-TOOL-0d69f4dbdda3 | tool | `skill` | `packages/opencode/src/tool/skill.ts:13` | unverified |
| BASELINE-TOOL-0e42fa6d83d6 | tool | `todowrite` | `packages/core/src/tool/todowrite.ts:12` | unverified |
| BASELINE-TOOL-58506aa1c233 | tool | `todowrite` | `packages/opencode/src/tool/todo.ts:15` | unverified |
| BASELINE-TOOL-8a36e6e0b0ac | tool | `webfetch` | `packages/core/src/tool/webfetch.ts:16` | unverified |
| BASELINE-TOOL-8fe0b3753d36 | tool | `webfetch` | `packages/opencode/src/tool/webfetch.ts:25` | unverified |
| BASELINE-TOOL-c58963ed4fe5 | tool | `websearch` | `packages/core/src/tool/websearch.ts:18` | unverified |
| BASELINE-TOOL-2632fd48b23a | tool | `websearch` | `packages/opencode/src/tool/websearch.ts:100` | unverified |
| BASELINE-TOOL-b0628fcc1980 | tool | `write` | `packages/core/src/tool/write.ts:19` | unverified |
| BASELINE-TOOL-9061e17d17e6 | tool | `write` | `packages/opencode/src/tool/write.ts:28` | unverified |
