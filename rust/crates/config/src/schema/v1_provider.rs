//! Purpose: legacy V1 `ConfigProviderV1.Info` and model shapes
//! (`packages/core/src/v1/config/provider.ts`) as decoder shapes.
//! Scope: schema values.
//! Uses:
//! rust/crates/
//! `-- config/src/schema/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- config/src/schema/v1.rs
//! Invariants: field sets and literals match the baseline provider/model schema.
//! Contract tests: config/tests/compat_fixtures.rs

use super::{Field, Shape, array, opt, record, req, strukt, union};

fn strings() -> Shape {
    array(Shape::String)
}

fn string_map() -> Shape {
    record(Shape::String)
}

fn modalities() -> Shape {
    array(Shape::Literals(&["text", "audio", "image", "video", "pdf"]))
}

fn cost_tier() -> Vec<Field> {
    vec![
        req("input", Shape::Finite),
        req("output", Shape::Finite),
        opt("cache_read", Shape::Finite),
        opt("cache_write", Shape::Finite),
    ]
}

fn provider_model() -> Shape {
    let interleaved_field = Shape::String;
    let mut cost = cost_tier();
    cost.push(opt("context_over_200k", strukt(cost_tier())));
    strukt(vec![
        opt("id", Shape::String),
        opt("name", Shape::String),
        opt("family", Shape::String),
        opt("release_date", Shape::String),
        opt("attachment", Shape::Bool),
        opt("reasoning", Shape::Bool),
        opt("temperature", Shape::Bool),
        opt("tool_call", Shape::Bool),
        opt(
            "interleaved",
            union(vec![
                Shape::Bool,
                interleaved_field.clone(),
                strukt(vec![req("field", interleaved_field)]),
            ]),
        ),
        opt("cost", strukt(cost)),
        opt(
            "limit",
            strukt(vec![
                req("context", Shape::Finite),
                opt("input", Shape::Finite),
                req("output", Shape::Finite),
            ]),
        ),
        opt(
            "modalities",
            strukt(vec![
                opt("input", modalities()),
                opt("output", modalities()),
            ]),
        ),
        opt("experimental", Shape::Bool),
        opt(
            "status",
            Shape::Literals(&["alpha", "beta", "deprecated", "active"]),
        ),
        opt(
            "provider",
            strukt(vec![opt("npm", Shape::String), opt("api", Shape::String)]),
        ),
        opt("options", record(Shape::Any)),
        opt("headers", string_map()),
        opt(
            "variants",
            record(Shape::Struct(
                vec![opt("disabled", Shape::Bool)],
                Some(Box::new(Shape::Any)),
            )),
        ),
    ])
}

pub fn provider() -> Shape {
    let timeout = || union(vec![Shape::PositiveInt, Shape::False]);
    let options = Shape::Struct(
        vec![
            opt("apiKey", Shape::String),
            opt("baseURL", Shape::String),
            opt("enterpriseUrl", Shape::String),
            opt("setCacheKey", Shape::Bool),
            opt("timeout", timeout()),
            opt("headerTimeout", timeout()),
            opt("chunkTimeout", timeout()),
        ],
        Some(Box::new(Shape::Any)),
    );
    strukt(vec![
        opt("api", Shape::String),
        opt("name", Shape::String),
        opt("env", strings()),
        opt("id", Shape::String),
        opt("npm", Shape::String),
        opt("whitelist", strings()),
        opt("blacklist", strings()),
        opt("options", options),
        opt("models", record(provider_model())),
    ])
}
