//! Purpose: in-sandbox value model (data, standard-library values, opaque runtime references).
//! Scope: value representation, identity, shared containers. No evaluation logic.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/value/convert.rs
//! |-- codemode/src/value/number.rs
//! |-- codemode/src/value/object.rs
//! `-- codemode/src/value/text.rs
//! External:
//! |-- f64::NAN
//! |-- indexmap::IndexMap
//! |-- regress::Regex
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- codemode/src/error.rs
//! |-- codemode/src/interp/call.rs
//! |-- codemode/src/interp/collections.rs
//! |-- codemode/src/interp/console.rs
//! |-- codemode/src/interp/diagnose.rs
//! |-- codemode/src/interp/expr.rs
//! |-- codemode/src/interp/members.rs
//! |-- codemode/src/interp/mod.rs
//! |-- codemode/src/interp/ops.rs
//! |-- codemode/src/interp/patterns.rs
//! |-- codemode/src/interp/pool.rs
//! |-- codemode/src/interp/promise.rs
//! |-- codemode/src/interp/stmt.rs
//! |-- codemode/src/lib.rs
//! |-- codemode/src/run.rs
//! |-- codemode/src/stdlib/array.rs
//! |-- codemode/src/stdlib/collections.rs
//! |-- codemode/src/stdlib/date.rs
//! |-- codemode/src/stdlib/json.rs
//! |-- codemode/src/stdlib/math.rs
//! |-- codemode/src/stdlib/mod.rs
//! |-- codemode/src/stdlib/number.rs
//! |-- codemode/src/stdlib/object.rs
//! |-- codemode/src/stdlib/regexp.rs
//! |-- codemode/src/stdlib/string.rs
//! |-- codemode/src/stdlib/url.rs
//! |-- codemode/src/stdlib/value.rs
//! |-- codemode/src/tools/runtime.rs
//! |-- codemode/src/value/convert.rs
//! `-- codemode/src/value/object.rs
//! Invariants: containers are shared by reference (JS identity), primitives by value; the
//! `ShortCircuit` marker never escapes an optional chain.
//! Contract tests: codemode/tests/parity.rs, codemode/tests/stdlib.rs

pub(crate) mod convert;
pub(crate) mod number;
pub(crate) mod object;
pub(crate) mod text;

use crate::ast::FunctionNode;
pub(crate) use object::{ArrayData, ObjectData};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

pub(crate) type Shared<T> = Arc<Mutex<T>>;

/// Locks a sandbox container. The interpreter is logically single-threaded, so poisoning only
/// follows a panic that already failed the execution; recover the data instead of panicking again.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn shared<T>(value: T) -> Shared<T> {
    Arc::new(Mutex::new(value))
}

/// A captured lexical scope.
pub(crate) type ScopeRef = Shared<indexmap::IndexMap<String, Binding>>;

#[derive(Clone)]
pub(crate) struct Binding {
    pub mutable: bool,
    pub value: Value,
    pub initialized: bool,
}

pub(crate) struct FunctionValue {
    pub node: Arc<FunctionNode>,
    pub captured: Vec<ScopeRef>,
}

pub(crate) struct RegExpData {
    pub source: String,
    pub flags: String,
    pub regex: regress::Regex,
    pub last_index: Mutex<f64>,
}

impl RegExpData {
    pub fn has(&self, flag: char) -> bool {
        self.flags.contains(flag)
    }
}

/// Map/Set key with SameValueZero semantics: primitives by value, containers by identity.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum MapKey {
    Undefined,
    Null,
    Bool(bool),
    Number(u64),
    Str(Arc<str>),
    Ref(usize),
}

#[derive(Default)]
pub(crate) struct MapData {
    pub entries: indexmap::IndexMap<MapKey, (Value, Value)>,
}

#[derive(Default)]
pub(crate) struct SetData {
    pub entries: indexmap::IndexMap<MapKey, Value>,
}

pub(crate) struct UrlData {
    pub url: url::Url,
    pub params: Shared<ParamsData>,
}

#[derive(Default)]
pub(crate) struct ParamsData {
    pub pairs: Vec<(String, String)>,
    /// Owning URL when these parameters are `url.searchParams` (mutations write through).
    pub owner: Option<std::sync::Weak<Mutex<UrlData>>>,
}

pub(crate) enum PromiseState {
    /// Backed by an eagerly started tool-call task in the execution's task pool.
    Task(usize),
    /// `Promise.resolve(value)` / `Promise.reject(reason)`.
    Ready(Result<Value, crate::error::Fault>),
}

pub(crate) struct PromiseData {
    pub state: PromiseState,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GlobalName {
    Object,
    Math,
    Json,
    Array,
    Console,
    Date,
    RegExp,
    Map,
    Set,
    Url,
    UrlSearchParams,
}

impl GlobalName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Object => "Object",
            Self::Math => "Math",
            Self::Json => "JSON",
            Self::Array => "Array",
            Self::Console => "console",
            Self::Date => "Date",
            Self::RegExp => "RegExp",
            Self::Map => "Map",
            Self::Set => "Set",
            Self::Url => "URL",
            Self::UrlSearchParams => "URLSearchParams",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Coercion {
    Number,
    String,
    Boolean,
    ParseInt,
    ParseFloat,
}

impl Coercion {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Number => "Number",
            Self::String => "String",
            Self::Boolean => "Boolean",
            Self::ParseInt => "parseInt",
            Self::ParseFloat => "parseFloat",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum UriFn {
    EncodeUri,
    EncodeUriComponent,
    DecodeUri,
    DecodeUriComponent,
}

impl UriFn {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EncodeUri => "encodeURI",
            Self::EncodeUriComponent => "encodeURIComponent",
            Self::DecodeUri => "decodeURI",
            Self::DecodeUriComponent => "decodeURIComponent",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PromiseMethod {
    All,
    AllSettled,
    Race,
    Resolve,
    Reject,
}

impl PromiseMethod {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "all" => Self::All,
            "allSettled" => Self::AllSettled,
            "race" => Self::Race,
            "resolve" => Self::Resolve,
            "reject" => Self::Reject,
            _ => return None,
        })
    }
}

pub(crate) const ERROR_CONSTRUCTORS: [&str; 7] = [
    "Error",
    "TypeError",
    "RangeError",
    "SyntaxError",
    "ReferenceError",
    "EvalError",
    "URIError",
];

#[derive(Clone)]
pub(crate) enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    Str(Arc<str>),
    Array(Shared<ArrayData>),
    Object(Shared<ObjectData>),
    Date(Arc<f64>),
    RegExp(Arc<RegExpData>),
    Map(Shared<MapData>),
    Set(Shared<SetData>),
    Url(Shared<UrlData>),
    Params(Shared<ParamsData>),
    Promise(Arc<PromiseData>),
    Function(Arc<FunctionValue>),
    Tool(Arc<Vec<String>>),
    Intrinsic(Arc<(Value, String)>),
    Global(GlobalName),
    GlobalMethod(Arc<(String, String)>),
    Coercion(Coercion),
    Uri(UriFn),
    ErrorCtor(&'static str),
    PromiseNs,
    PromiseMethod(PromiseMethod),
    /// Optional-chain short circuit marker; converted to `undefined` by the chain expression.
    ShortCircuit,
}

impl Value {
    pub fn str(text: impl AsRef<str>) -> Self {
        Self::Str(Arc::from(text.as_ref()))
    }

    pub fn array(items: Vec<Value>) -> Self {
        Self::Array(shared(ArrayData::new(items)))
    }

    pub fn object(data: ObjectData) -> Self {
        Self::Object(shared(data))
    }

    pub fn is_nullish(&self) -> bool {
        matches!(self, Self::Undefined | Self::Null)
    }

    /// Standard-library value instances (Date, RegExp, Map, Set, URL, URLSearchParams).
    pub fn is_sandbox(&self) -> bool {
        matches!(
            self,
            Self::Date(_)
                | Self::RegExp(_)
                | Self::Map(_)
                | Self::Set(_)
                | Self::Url(_)
                | Self::Params(_)
        )
    }

    /// Opaque interpreter machinery or sandbox values (`isRuntimeReference`).
    pub fn is_runtime_reference(&self) -> bool {
        self.is_sandbox() || self.is_opaque_reference()
    }

    /// Opaque interpreter machinery (functions, tools, promises, intrinsic references).
    pub fn is_opaque_reference(&self) -> bool {
        matches!(
            self,
            Self::Promise(_)
                | Self::Function(_)
                | Self::Tool(_)
                | Self::Intrinsic(_)
                | Self::Global(_)
                | Self::GlobalMethod(_)
                | Self::Coercion(_)
                | Self::Uri(_)
                | Self::ErrorCtor(_)
                | Self::PromiseNs
                | Self::PromiseMethod(_)
        )
    }

    /// `typeof x === "object" && x !== null` for data and sandbox values.
    pub fn is_object_like(&self) -> bool {
        matches!(self, Self::Array(_) | Self::Object(_) | Self::Promise(_)) || self.is_sandbox()
    }

    pub fn truthy(&self) -> bool {
        match self {
            Self::Undefined | Self::Null | Self::ShortCircuit => false,
            Self::Bool(value) => *value,
            Self::Number(value) => !(value.is_nan() || *value == 0.0),
            Self::Str(value) => !value.is_empty(),
            _ => true,
        }
    }

    /// Identity of a reference value, used for `===`, Map keys and cycle detection.
    pub fn identity(&self) -> Option<usize> {
        Some(match self {
            Self::Array(value) => Arc::as_ptr(value) as *const () as usize,
            Self::Object(value) => Arc::as_ptr(value) as *const () as usize,
            Self::Date(value) => Arc::as_ptr(value) as *const () as usize,
            Self::RegExp(value) => Arc::as_ptr(value) as *const () as usize,
            Self::Map(value) => Arc::as_ptr(value) as *const () as usize,
            Self::Set(value) => Arc::as_ptr(value) as *const () as usize,
            Self::Url(value) => Arc::as_ptr(value) as *const () as usize,
            Self::Params(value) => Arc::as_ptr(value) as *const () as usize,
            Self::Promise(value) => Arc::as_ptr(value) as *const () as usize,
            Self::Function(value) => Arc::as_ptr(value) as *const () as usize,
            Self::Tool(value) => Arc::as_ptr(value) as *const () as usize,
            Self::Intrinsic(value) => Arc::as_ptr(value) as *const () as usize,
            Self::GlobalMethod(value) => Arc::as_ptr(value) as *const () as usize,
            _ => return None,
        })
    }

    pub fn map_key(&self) -> MapKey {
        match self {
            Self::Undefined | Self::ShortCircuit => MapKey::Undefined,
            Self::Null => MapKey::Null,
            Self::Bool(value) => MapKey::Bool(*value),
            Self::Number(value) => MapKey::Number(if value.is_nan() {
                f64::NAN.to_bits()
            } else if *value == 0.0 {
                0f64.to_bits()
            } else {
                value.to_bits()
            }),
            Self::Str(value) => MapKey::Str(value.clone()),
            Self::Global(name) => MapKey::Str(Arc::from(format!("\u{0}global:{}", name.as_str()))),
            Self::Coercion(name) => {
                MapKey::Str(Arc::from(format!("\u{0}coerce:{}", name.as_str())))
            }
            Self::Uri(name) => MapKey::Str(Arc::from(format!("\u{0}uri:{}", name.as_str()))),
            Self::ErrorCtor(name) => MapKey::Str(Arc::from(format!("\u{0}error:{name}"))),
            Self::PromiseNs => MapKey::Str(Arc::from("\u{0}promise")),
            Self::PromiseMethod(name) => MapKey::Str(Arc::from(format!("\u{0}promise:{name:?}"))),
            other => MapKey::Ref(other.identity().unwrap_or_default()),
        }
    }

    /// `===` (strict equality).
    pub fn strict_equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Self::Undefined, Self::Undefined) | (Self::Null, Self::Null) => true,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::Number(left), Self::Number(right)) => left == right,
            (Self::Str(left), Self::Str(right)) => left == right,
            (Self::Global(left), Self::Global(right)) => left == right,
            (Self::Coercion(left), Self::Coercion(right)) => left == right,
            (Self::Uri(left), Self::Uri(right)) => left == right,
            (Self::ErrorCtor(left), Self::ErrorCtor(right)) => left == right,
            (Self::PromiseNs, Self::PromiseNs) => true,
            (Self::PromiseMethod(left), Self::PromiseMethod(right)) => left == right,
            (left, right) => match (left.identity(), right.identity()) {
                (Some(left), Some(right)) => left == right,
                _ => false,
            },
        }
    }

    /// SameValueZero (Array.includes, Map/Set keys).
    pub fn same_value_zero(&self, other: &Value) -> bool {
        match (self, other) {
            (Self::Number(left), Self::Number(right)) if left.is_nan() && right.is_nan() => true,
            _ => self.strict_equals(other),
        }
    }
}

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

/// Process-unique id for task bookkeeping.
pub(crate) fn next_id() -> usize {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}
