# Terminal editor and rendering observations, 2026-10-19

Owner: ant. Scope: `ui-terminal/src/prompt/**`, `timeline/**`, `rendering.rs`, matching public tests. App/routes/dialogs/bin belong to herb. This is an implemented and tested slice, **not full terminal product parity**.

## Baseline to observation

| Baseline requirement | Implementation | Observed public check |
|---|---|---|
| Prompt editing uses grapheme boundaries, not UTF-8 bytes or scalar deletion | `prompt::Editor`, segmentation on insertion/deletion/motion/selection | `editor.rs`: family emoji, skin tone, flags, combining marks, Indic and CJK, insertion joining clusters, undo/redo selection replacement |
| CRLF/CR multiline paste inserts without submitting | `Editor::paste`, `handle(UiEvent::Paste)` | multiline normalized text remains editable; Enter alone submits; shifted Enter inserts newline; released keys ignored |
| Logical/display/visual columns differ, wide clusters remain intact | `prompt/layout.rs`, `handle_at`, preferred column | wide cluster layout, logical movement through short lines, wrapped visual movement, exact-margin newline, visual Home/End |
| Display offsets preserve newline positions for prompt virtual regions | `offset_width`, `display_slice`, `mention_trigger_index` | baseline mention examples, newline count one, tab/CRLF count zero, leading zero-width text preserved; one initial CRLF mismatch was fixed rather than excluded |
| Default editing bindings and history protect draft state | `prompt/keys.rs`, command IDs, bounded history accessors | buffer versus logical/visual keys, undo/redo, word/delete commands, newest-50 consecutive dedupe, history draft protection and persistence seeding |
| Markdown renders incrementally rather than only after final response | `timeline/markdown.rs`, `Timeline::append` | headings, nested modifiers, links, code, lists/tasks, table separators and unfinished fenced code through real public rendering |
| Diffs preserve old/new identity and missing terminal newline | `timeline/diff.rs` | numbered before/after changes, colors, newline marker; existing unified patch headers/additions/deletions retained |
| Actual V2 and legacy messages preserve tool lifecycle and output | `Message::from_value`, strict `ProjectionError` | all four tool states, structured metadata/content, text/reasoning, attachments/shell/model switch/compaction, invalid and unsupported content fail explicitly |
| Tool-specific metadata affects visible content | `timeline/tool_details.rs` | read loaded paths, edit patch and diagnostics, apply-patch file titles, todo marks, question answers and execute child status; pending metadata hidden |
| Terminal output updates cells without clearing every streaming frame | `rendering::Renderer` with ratatui buffer diff | actual emitted ANSI parsed by vt100: wide/combining cells, cursor, incremental repaint, resize, tiny viewports, overlay and control stripping |

## Commands and results

All Cargo invocations use the shared `repair-env.sh` wrapper, pinned cache, `--locked --offline`.

- Initial `cargo test -p opencode-ui-terminal` plus `cargo clippy -p opencode-ui-terminal --all-targets --no-deps -- -D warnings` passed after fixing owned lints and coordinating a foreign dialog lint with herb.
- Latest full UI test run completed successfully on 2026-10-19 at 11:36 UTC, including 14 editor, 8 renderer, 4 projection and 5 specialized tool tests, plus the existing input/keybind/theme/terminal/app suites.
- The immediately following strict Clippy invocation was initially blocked by a concurrent workspace manifest/lock change. After root refreshed its owned lock, the rerun completed successfully at 11:47 UTC with `--locked --offline --all-targets --no-deps -- -D warnings`.
- Existing package terminal PTY tests passed. They are not the same as exercising the executable app with these new editor APIs.
- Executable Unicode/paste PTY integration is owned by herb. At 11:32 UTC herb reported provider-adapter compilation errors blocking it; bird owns those fixes. No live executable acceptance is claimed here.

## Explicit remaining baseline gaps

The source review covered prompt submit/paste/history/display/editor producers and session text/reasoning/tool renderers. The complete autocomplete implementation and every navigation producer were not covered by this worker.

- Prompt virtual parts, large-paste summaries, attachment editor/frecency/autocomplete/shell-mode/stash metadata and complete PromptInfo history are not implemented by this plain-text editor. History accessors intentionally expose text only; herb was warned not to discard legacy parts/mode during persistence.
- Markdown has no syntax highlighter, exact table grid layout or theme-equivalent appearance. Unified patches are colored, not a full parsed split diff with wrap toggles.
- Specialized widgets cover verified metadata above, not every TS action, permission affordance, task navigation/retry, output character cap, reasoning summary/elapsed-time or click callback.
- Selection/cursor behavior is tested through the public model and ANSI screen, but exhaustive baseline bindings and production terminal emulator differences remain unverified.
- No Windows/Linux runtime claim, no installed dependencies, no user data or manifest edits by this worker.
