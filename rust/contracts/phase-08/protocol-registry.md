# Note for p08 children: protocol registry is available

Phase 06 (wyvern) landed opencode-protocol (commits 656a7cafe, a3f2654a2), wasm32 clean.

- `opencode_protocol::registry() -> &'static [&'static Api]`: `[0]` current Server API, `[1]` legacy PublicApi. Also `current::api()`, `legacy::api()`.
- `operation.rs`: `Api{identifier,title,version,description,groups,definitions}`, `Group{identifier,title,description,top_level,legacy,endpoints}`, `Endpoint{name,operation_id,method,path(":param", "*"),params,query,headers,deep_object_query,payload,payload_encoding,payload_required,successes{status,schema,encoding json|sse|void|bytes|webSocket},errors{status,schema},middleware,summary,description,deprecated}`.
- `ir.rs`: structural `Schema` + `Field{name,optionality Required|Optional|Exact,schema,description}` + `Definition` (registration order).
- `errors.rs`, `sse.rs` (`Frame`, `Decoder`), `openapi::document(&[&Api])` slot.
- Population: only health+location at first; all 18 current groups + legacy are being filled with the same types.

Rules for p08: codegen-ir implements a `Source` adapter over this registry (primary) and keeps the TS-captured JSON snapshot as a parity oracle. Client aliases/omissions are applied by p08, not p06. Requested additive IR changes (Deserialize, StructWithRest, Never, Field.default, Checks.format etc.) were sent to wyvern; ask wyvern directly for further additive variants, never edit crates/protocol.
