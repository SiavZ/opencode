//! Purpose: `URL`, `URLSearchParams` and the URI encoding helpers.
//! Scope: stdlib/url.ts plus runtime.ts constructURL/constructURLSearchParams/
//! invokeURLSearchParamsMethod/URL member writes.
//! Uses:
//! rust/crates/
//! |-- codemode/src/value/convert.rs
//! `-- codemode/src/value/mod.rs
//! External:
//! |-- form_urlencoded
//! |-- percent_encoding
//! `-- url::{Url, quirks}
//! Used by:
//! rust/crates/
//! |-- codemode/src/interp/call.rs
//! |-- codemode/src/interp/collections.rs
//! `-- codemode/src/interp/members.rs
//! Invariants: `url.searchParams` is live (mutations write through, URL writes re-parse);
//! parsing never touches the network.
//! Contract tests: codemode/tests/stdlib.rs

use crate::ast::Loc;
use crate::error::{Eval, Fault, fail, fail_named};
use crate::value::convert::{bounded, to_string};
use crate::value::{ParamsData, Shared, UriFn, UrlData, Value, lock, shared};
use std::sync::Arc;
use url::{Url, quirks};

pub(crate) const URL_PROPERTIES: &[&str] = &[
    "href", "origin", "protocol", "username", "password", "host", "hostname", "port", "pathname",
    "search", "hash",
];
pub(crate) const URL_METHODS: &[&str] = &["toString", "toJSON"];
pub(crate) const PARAMS_METHODS: &[&str] = &[
    "append", "delete", "get", "getAll", "has", "set", "sort", "forEach", "keys", "values",
    "entries", "toString",
];

pub(crate) fn serialize_params(pairs: &[(String, String)]) -> String {
    form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

fn parse_params(query: &str) -> Vec<(String, String)> {
    let query = query.strip_prefix('?').unwrap_or(query);
    form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

fn argument(value: &Value, label: &str) -> Eval<String> {
    Ok(to_string(&bounded(value, label)?))
}

fn url_argument(value: &Value, label: &str) -> Eval<String> {
    match value {
        Value::Url(data) => Ok(lock(data).url.as_str().to_string()),
        other => argument(other, label),
    }
}

fn new_url(url: Url) -> Value {
    let pairs = parse_params(url.query().unwrap_or(""));
    let params = shared(ParamsData { pairs, owner: None });
    let data = Arc::new(std::sync::Mutex::new(UrlData {
        url,
        params: params.clone(),
    }));
    lock(&params).owner = Some(Arc::downgrade(&data));
    Value::Url(data)
}

fn parse_url(input: &str, base: Option<&str>) -> Option<Url> {
    match base {
        Some(base) => Url::parse(base).ok()?.join(input).ok(),
        None => Url::parse(input).ok(),
    }
}

pub(crate) fn construct_url(args: &[Value], loc: Loc) -> Eval<Value> {
    if args.is_empty() {
        return Err(fail_named(
            "new URL(...) requires a URL string and an optional base URL.",
            loc,
            "TypeError",
        ));
    }
    let input = url_argument(&args[0], "new URL input")?;
    let base = match args.get(1) {
        None | Some(Value::Undefined) => None,
        Some(value) => Some(url_argument(value, "new URL base")?),
    };
    parse_url(&input, base.as_deref())
        .map(new_url)
        .ok_or_else(|| {
            let suffix = if base.is_some() { " or base URL" } else { "" };
            fail_named(
                format!("new URL(...) received an invalid URL{suffix}."),
                loc,
                "TypeError",
            )
        })
}

pub(crate) fn construct_params(init: Option<&Value>, loc: Loc) -> Eval<Value> {
    let pairs = match init {
        None | Some(Value::Undefined) => Vec::new(),
        Some(Value::Params(params)) => lock(params).pairs.clone(),
        Some(Value::Str(text)) => parse_params(text),
        Some(value @ (Value::Null | Value::Number(_) | Value::Bool(_))) => {
            parse_params(&to_string(value))
        }
        Some(Value::Map(map)) => {
            let entries: Vec<(Value, Value)> = lock(map).entries.values().cloned().collect();
            entries
                .iter()
                .map(|(key, item)| {
                    Ok((
                        argument(key, "URLSearchParams name")?,
                        argument(item, "URLSearchParams value")?,
                    ))
                })
                .collect::<Eval<_>>()?
        }
        Some(Value::Array(array)) => {
            let items = lock(array).items.clone();
            items
                .iter()
                .map(|pair| {
                    let Value::Array(pair) = pair else {
                        return Err(pairs_error(loc));
                    };
                    let pair = lock(pair).items.clone();
                    if pair.len() != 2 {
                        return Err(pairs_error(loc));
                    }
                    Ok((
                        argument(&pair[0], "URLSearchParams name")?,
                        argument(&pair[1], "URLSearchParams value")?,
                    ))
                })
                .collect::<Eval<_>>()?
        }
        Some(value) if value.is_sandbox() => Vec::new(),
        Some(value) => match bounded(value, "new URLSearchParams input")? {
            Value::Object(object) => lock(&object)
                .entries()
                .into_iter()
                .map(|(key, item)| (key, to_string(&item)))
                .collect(),
            _ => {
                return Err(fail_named(
                    "new URLSearchParams(...) expects a query string, data object, array of pairs, or URLSearchParams.",
                    loc,
                    "TypeError",
                ));
            }
        },
    };
    Ok(Value::Params(shared(ParamsData { pairs, owner: None })))
}

fn pairs_error(loc: Loc) -> Fault {
    fail_named(
        "new URLSearchParams(...) expects an array of [name, value] pairs.",
        loc,
        "TypeError",
    )
}

pub(crate) fn invoke_static(name: &str, args: &[Value], loc: Loc) -> Eval<Value> {
    if name != "canParse" && name != "parse" {
        return Err(fail(
            format!("URL.{name} is not available in CodeMode."),
            loc,
        ));
    }
    if args.is_empty() {
        return Err(fail_named(
            format!("URL.{name} requires a URL argument."),
            loc,
            "TypeError",
        ));
    }
    let input = url_argument(&args[0], &format!("URL.{name} input"))?;
    let base = match args.get(1) {
        None | Some(Value::Undefined) => None,
        Some(value) => Some(url_argument(value, &format!("URL.{name} base"))?),
    };
    let parsed = parse_url(&input, base.as_deref());
    Ok(match (name, parsed) {
        ("canParse", parsed) => Value::Bool(parsed.is_some()),
        (_, Some(url)) => new_url(url),
        (_, None) => Value::Null,
    })
}

pub(crate) fn invoke_url_method(data: &UrlData, name: &str, loc: Loc) -> Eval<Value> {
    if URL_METHODS.contains(&name) {
        return Ok(Value::str(data.url.as_str()));
    }
    Err(fail(
        format!("URL method '{name}' is not available in CodeMode."),
        loc,
    ))
}

pub(crate) fn get_property(data: &UrlData, name: &str) -> Value {
    let url = &data.url;
    Value::str(match name {
        "href" => quirks::href(url).to_string(),
        "origin" => quirks::origin(url),
        "protocol" => quirks::protocol(url).to_string(),
        "username" => quirks::username(url).to_string(),
        "password" => quirks::password(url).to_string(),
        "host" => quirks::host(url).to_string(),
        "hostname" => quirks::hostname(url).to_string(),
        "port" => quirks::port(url).to_string(),
        "pathname" => quirks::pathname(url).to_string(),
        "search" => quirks::search(url).to_string(),
        "hash" => quirks::hash(url).to_string(),
        _ => return Value::Undefined,
    })
}

pub(crate) fn set_property(
    data: &Shared<UrlData>,
    name: &str,
    value: &Value,
    loc: Loc,
) -> Eval<()> {
    if name == "origin" {
        return Err(fail_named(
            format!("URL.{name} is read-only."),
            loc,
            "TypeError",
        ));
    }
    let text = argument(value, &format!("URL.{name} value"))?;
    let mut guard = lock(data);
    let url = &mut guard.url;
    // Like the WHATWG setters: only `href` rejects invalid input; the others ignore it.
    match name {
        "href" => {
            if quirks::set_href(url, &text).is_err() {
                return Err(fail_named(
                    format!("URL.{name} received an invalid value."),
                    loc,
                    "TypeError",
                ));
            }
        }
        "protocol" => drop(quirks::set_protocol(url, &text)),
        "username" => drop(quirks::set_username(url, &text)),
        "password" => drop(quirks::set_password(url, &text)),
        "host" => drop(quirks::set_host(url, &text)),
        "hostname" => drop(quirks::set_hostname(url, &text)),
        "port" => drop(quirks::set_port(url, &text)),
        "pathname" => quirks::set_pathname(url, &text),
        "search" => quirks::set_search(url, &text),
        "hash" => quirks::set_hash(url, &text),
        _ => {}
    }
    let pairs = parse_params(guard.url.query().unwrap_or(""));
    lock(&guard.params).pairs = pairs;
    Ok(())
}

fn write_through(params: &ParamsData) {
    if let Some(owner) = params.owner.as_ref().and_then(std::sync::Weak::upgrade) {
        let serialized = serialize_params(&params.pairs);
        lock(&owner).url.set_query(if serialized.is_empty() {
            None
        } else {
            Some(&serialized)
        });
    }
}

pub(crate) fn params_method(
    params: &Shared<ParamsData>,
    name: &str,
    args: &[Value],
    loc: Loc,
) -> Eval<Value> {
    let require = |count: usize| -> Eval<()> {
        if args.len() < count {
            let plural = if count == 1 { "" } else { "s" };
            return Err(fail_named(
                format!("URLSearchParams.{name} requires {count} argument{plural}."),
                loc,
                "TypeError",
            ));
        }
        Ok(())
    };
    let arg = |index: usize| {
        argument(
            &args[index],
            &format!("URLSearchParams.{name} argument {}", index + 1),
        )
    };
    let explicit_value =
        |index: usize| matches!(args.get(index), Some(value) if !matches!(value, Value::Undefined));
    let mut data = lock(params);
    let result = match name {
        "append" => {
            require(2)?;
            data.pairs.push((arg(0)?, arg(1)?));
            Value::Undefined
        }
        "delete" => {
            require(1)?;
            let key = arg(0)?;
            let value = if explicit_value(1) {
                Some(arg(1)?)
            } else {
                None
            };
            data.pairs.retain(|(name, item)| {
                !(*name == key && value.as_ref().is_none_or(|value| item == value))
            });
            Value::Undefined
        }
        "get" => {
            require(1)?;
            let key = arg(0)?;
            return Ok(data
                .pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map_or(Value::Null, |(_, item)| Value::str(item)));
        }
        "getAll" => {
            require(1)?;
            let key = arg(0)?;
            return Ok(Value::array(
                data.pairs
                    .iter()
                    .filter(|(name, _)| *name == key)
                    .map(|(_, item)| Value::str(item))
                    .collect(),
            ));
        }
        "has" => {
            require(1)?;
            let key = arg(0)?;
            let value = if explicit_value(1) {
                Some(arg(1)?)
            } else {
                None
            };
            return Ok(Value::Bool(data.pairs.iter().any(|(name, item)| {
                *name == key && value.as_ref().is_none_or(|value| item == value)
            })));
        }
        "set" => {
            require(2)?;
            let (key, value) = (arg(0)?, arg(1)?);
            match data.pairs.iter().position(|(name, _)| *name == key) {
                Some(first) => {
                    data.pairs[first].1 = value;
                    let mut index = 0;
                    data.pairs.retain(|(name, _)| {
                        index += 1;
                        index - 1 == first || *name != key
                    });
                }
                None => data.pairs.push((key, value)),
            }
            Value::Undefined
        }
        "sort" => {
            data.pairs
                .sort_by(|left, right| left.0.encode_utf16().cmp(right.0.encode_utf16()));
            Value::Undefined
        }
        "keys" => {
            return Ok(Value::array(
                data.pairs
                    .iter()
                    .map(|(name, _)| Value::str(name))
                    .collect(),
            ));
        }
        "values" => {
            return Ok(Value::array(
                data.pairs
                    .iter()
                    .map(|(_, item)| Value::str(item))
                    .collect(),
            ));
        }
        "entries" => {
            return Ok(Value::array(
                data.pairs
                    .iter()
                    .map(|(name, item)| Value::array(vec![Value::str(name), Value::str(item)]))
                    .collect(),
            ));
        }
        "toString" => return Ok(Value::str(serialize_params(&data.pairs))),
        _ => {
            return Err(fail(
                format!("URLSearchParams method '{name}' is not available in CodeMode."),
                loc,
            ));
        }
    };
    write_through(&data);
    Ok(result)
}

const URI_RESERVED: &str = ";/?:@&=+$,#";
const URI_UNESCAPED_MARKS: &str = "-_.!~*'()";

fn encode(text: &str, keep: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || URI_UNESCAPED_MARKS.contains(ch) || keep.contains(ch) {
            out.push(ch);
        } else {
            let mut buffer = [0u8; 4];
            for byte in ch.encode_utf8(&mut buffer).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

fn decode(text: &str, preserve: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            out.push(bytes[index]);
            index += 1;
            continue;
        }
        let hex = text.get(index + 1..index + 3)?;
        let byte = u8::from_str_radix(hex, 16).ok()?;
        if byte < 0x80 && preserve.contains(byte as char) {
            out.extend_from_slice(&bytes[index..index + 3]);
        } else {
            out.push(byte);
        }
        index += 3;
    }
    String::from_utf8(out).ok()
}

pub(crate) fn invoke_uri(function: UriFn, args: &[Value], loc: Loc) -> Eval<Value> {
    let value = argument(
        args.first().unwrap_or(&Value::Undefined),
        &format!("{} input", function.as_str()),
    )?;
    let result = match function {
        UriFn::EncodeUri => Some(encode(&value, URI_RESERVED)),
        UriFn::EncodeUriComponent => Some(encode(&value, "")),
        UriFn::DecodeUri => decode(&value, URI_RESERVED),
        UriFn::DecodeUriComponent => decode(&value, ""),
    };
    result.map(Value::str).ok_or_else(|| {
        fail_named(
            format!(
                "{} received malformed URI data: URI malformed",
                function.as_str()
            ),
            loc,
            "URIError",
        )
    })
}
