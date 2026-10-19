//! Purpose: owned, lifetime-free AST for the supported CodeMode syntax subset, lowered from oxc.
//! Scope: node shapes only. Unsupported constructs are kept as `Unsupported` nodes so the
//! diagnostic is raised lazily at evaluation time, like the baseline interpreter.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! |-- codemode/src/error.rs
//! |-- codemode/src/interp/call.rs
//! |-- codemode/src/interp/collections.rs
//! |-- codemode/src/interp/console.rs
//! |-- codemode/src/interp/diagnose.rs
//! |-- codemode/src/interp/expr.rs
//! |-- codemode/src/interp/members.rs
//! |-- codemode/src/interp/members/cycles.rs
//! |-- codemode/src/interp/mod.rs
//! |-- codemode/src/interp/ops.rs
//! |-- codemode/src/interp/patterns.rs
//! |-- codemode/src/interp/promise.rs
//! |-- codemode/src/interp/stmt.rs
//! |-- codemode/src/lib.rs
//! |-- codemode/src/parse/expr.rs
//! |-- codemode/src/parse/mod.rs
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
//! `-- codemode/src/value/mod.rs
//! Invariants: `Loc` is 1-based (line, column) in the user's source.
//! Contract tests: codemode/tests/codemode.rs

use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Loc {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug)]
pub(crate) enum Literal {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
}

#[derive(Debug)]
pub(crate) enum Stmt {
    Expr(Expr),
    Var(VarDecl),
    Return(Option<Expr>),
    Block(Vec<Stmt>),
    If {
        test: Expr,
        consequent: Box<Stmt>,
        alternate: Option<Box<Stmt>>,
    },
    Switch {
        discriminant: Expr,
        cases: Vec<SwitchCase>,
        loc: Loc,
    },
    While {
        test: Expr,
        body: Box<Stmt>,
    },
    DoWhile {
        body: Box<Stmt>,
        test: Expr,
    },
    For {
        init: Option<ForInit>,
        test: Option<Expr>,
        update: Option<Expr>,
        body: Box<Stmt>,
    },
    ForOf(Box<ForEach>),
    ForIn(Box<ForEach>),
    Break {
        labeled: bool,
        loc: Loc,
    },
    Continue {
        labeled: bool,
        loc: Loc,
    },
    Throw(Expr),
    Try {
        block: Box<Stmt>,
        handler: Option<CatchClause>,
        finalizer: Option<Box<Stmt>>,
    },
    Empty,
    Function {
        name: String,
        function: Arc<FunctionNode>,
        loc: Loc,
    },
    Unsupported {
        kind: String,
        loc: Loc,
    },
}

#[derive(Debug)]
pub(crate) struct CatchClause {
    pub param: Option<Pattern>,
    pub body: Box<Stmt>,
}

#[derive(Debug)]
pub(crate) struct SwitchCase {
    pub test: Option<Expr>,
    pub consequent: Vec<Stmt>,
}

#[derive(Debug)]
pub(crate) enum ForInit {
    Var(VarDecl),
    Expr(Expr),
}

#[derive(Debug)]
pub(crate) enum ForLeft {
    Declare {
        pattern: Pattern,
        mutable: bool,
        count: usize,
        loc: Loc,
    },
    Assign {
        name: String,
        loc: Loc,
    },
    Unsupported(Loc),
}

#[derive(Debug)]
pub(crate) struct ForEach {
    pub left: ForLeft,
    pub right: Expr,
    pub body: Stmt,
    pub is_await: bool,
    pub loc: Loc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VarKind {
    Var,
    Let,
    Const,
}

#[derive(Debug)]
pub(crate) struct VarDecl {
    pub kind: VarKind,
    pub declarations: Vec<(Pattern, Option<Expr>, Loc)>,
}

#[derive(Debug)]
pub(crate) enum Pattern {
    Ident(String, Loc),
    Default {
        left: Box<Pattern>,
        right: Box<Expr>,
        loc: Loc,
    },
    Object {
        properties: Vec<PatternProperty>,
        rest: Option<Box<Pattern>>,
        loc: Loc,
    },
    Array {
        elements: Vec<Option<Pattern>>,
        rest: Option<Box<Pattern>>,
        loc: Loc,
    },
    Unsupported {
        kind: String,
        loc: Loc,
    },
}

#[derive(Debug)]
pub(crate) struct PatternProperty {
    /// `None` for computed keys, which CodeMode rejects at evaluation time.
    pub key: Option<String>,
    pub key_loc: Loc,
    pub value: Pattern,
    pub loc: Loc,
}

#[derive(Debug)]
pub(crate) enum FunctionBody {
    Block(Stmt),
    Expr(Expr),
}

#[derive(Debug)]
pub(crate) struct FunctionNode {
    pub params: Vec<Pattern>,
    pub rest: Option<Pattern>,
    pub body: FunctionBody,
    pub generator: bool,
    pub loc: Loc,
}

#[derive(Debug)]
pub(crate) enum Arg {
    Expr(Expr),
    Spread(Expr, Loc),
}

#[derive(Debug)]
pub(crate) enum PropKey {
    Ident(String),
    Literal(Literal),
    Computed(Expr),
}

#[derive(Debug)]
pub(crate) enum ObjectProp {
    Init {
        key: PropKey,
        key_loc: Loc,
        value: Expr,
    },
    Spread(Expr, Loc),
    Accessor(Loc),
}

#[derive(Debug)]
pub(crate) enum MemberProp {
    Static(String, Loc),
    Computed(Box<Expr>, Loc),
}

#[derive(Debug)]
pub(crate) struct Member {
    pub object: Expr,
    pub property: MemberProp,
    pub optional: bool,
    pub loc: Loc,
}

#[derive(Debug)]
pub(crate) enum AssignTarget {
    Ident(String, Loc),
    Member(Box<Member>),
    Invalid(Loc),
}

#[derive(Debug)]
pub(crate) enum Expr {
    Literal(Literal, Loc),
    BigInt(Loc),
    Regex {
        pattern: String,
        flags: String,
        loc: Loc,
    },
    Ident(String, Loc),
    Binary {
        op: &'static str,
        left: Box<Expr>,
        right: Box<Expr>,
        loc: Loc,
    },
    Logical {
        op: &'static str,
        left: Box<Expr>,
        right: Box<Expr>,
        loc: Loc,
    },
    Unary {
        op: &'static str,
        argument: Box<Expr>,
        loc: Loc,
    },
    Assign {
        op: &'static str,
        target: AssignTarget,
        right: Box<Expr>,
        loc: Loc,
    },
    Update {
        increment: bool,
        prefix: bool,
        target: AssignTarget,
        loc: Loc,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
        optional: bool,
        loc: Loc,
    },
    New {
        callee: Box<Expr>,
        args: Vec<Arg>,
        loc: Loc,
    },
    Member(Box<Member>),
    Chain(Box<Expr>),
    Object(Vec<ObjectProp>, Loc),
    Array(Vec<Option<Arg>>, Loc),
    Template {
        quasis: Vec<Option<String>>,
        expressions: Vec<Expr>,
        loc: Loc,
    },
    Conditional {
        test: Box<Expr>,
        consequent: Box<Expr>,
        alternate: Box<Expr>,
    },
    Await(Box<Expr>, Loc),
    Function(Arc<FunctionNode>),
    Unsupported {
        kind: String,
        loc: Loc,
    },
}

impl Expr {
    pub fn loc(&self) -> Loc {
        match self {
            Self::Literal(_, loc)
            | Self::BigInt(loc)
            | Self::Regex { loc, .. }
            | Self::Ident(_, loc)
            | Self::Binary { loc, .. }
            | Self::Logical { loc, .. }
            | Self::Unary { loc, .. }
            | Self::Assign { loc, .. }
            | Self::Update { loc, .. }
            | Self::Call { loc, .. }
            | Self::New { loc, .. }
            | Self::Object(_, loc)
            | Self::Array(_, loc)
            | Self::Template { loc, .. }
            | Self::Await(_, loc)
            | Self::Unsupported { loc, .. } => *loc,
            Self::Member(member) => member.loc,
            Self::Chain(inner) => inner.loc(),
            Self::Conditional { test, .. } => test.loc(),
            Self::Function(function) => function.loc,
        }
    }
}

impl Pattern {
    pub fn loc(&self) -> Loc {
        match self {
            Self::Ident(_, loc)
            | Self::Default { loc, .. }
            | Self::Object { loc, .. }
            | Self::Array { loc, .. }
            | Self::Unsupported { loc, .. } => *loc,
        }
    }

    /// Every identifier the pattern binds, used to seed TDZ slots before defaults run.
    pub fn names(&self, out: &mut Vec<String>) {
        match self {
            Self::Ident(name, _) => out.push(name.clone()),
            Self::Default { left, .. } => left.names(out),
            Self::Object {
                properties, rest, ..
            } => {
                for property in properties {
                    property.value.names(out);
                }
                if let Some(rest) = rest {
                    rest.names(out);
                }
            }
            Self::Array { elements, rest, .. } => {
                for element in elements.iter().flatten() {
                    element.names(out);
                }
                if let Some(rest) = rest {
                    rest.names(out);
                }
            }
            Self::Unsupported { .. } => {}
        }
    }
}
