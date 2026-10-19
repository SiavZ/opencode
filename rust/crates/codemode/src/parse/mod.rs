//! Purpose: parse CodeMode source with oxc and lower it into the owned AST.
//! Scope: program/statement/pattern lowering; expressions live in `parse/expr.rs`. TypeScript
//! syntax is erased (type-only declarations become empty statements), matching the baseline's
//! TypeScript transpile step.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/parse/expr.rs
//! `-- codemode/src/value/number.rs
//! External:
//! |-- ast::BindingPattern
//! |-- ast::BindingPattern::ArrayPattern
//! |-- ast::BindingPattern::AssignmentPattern
//! |-- ast::BindingPattern::BindingIdentifier
//! |-- ast::BindingPattern::ObjectPattern
//! |-- ast::ForStatementInit::VariableDeclaration
//! |-- ast::ForStatementLeft
//! |-- ast::ForStatementLeft::AssignmentTargetIdentifier
//! |-- ast::ForStatementLeft::VariableDeclaration
//! |-- ast::FormalParameters
//! |-- ast::Function
//! |-- ast::PropertyKey
//! |-- ast::PropertyKey::NumericLiteral
//! |-- ast::PropertyKey::StaticIdentifier
//! |-- ast::PropertyKey::StringLiteral
//! |-- ast::Statement
//! |-- ast::VariableDeclaration
//! |-- ast::VariableDeclarationKind::Const
//! |-- ast::VariableDeclarationKind::Let
//! |-- ast::VariableDeclarationKind::Var
//! |-- oxc_allocator::Allocator
//! |-- oxc_ast::ast
//! |-- oxc_parser::ParseOptions
//! |-- oxc_parser::Parser
//! |-- oxc_span::GetSpan::span
//! |-- oxc_span::SourceType
//! `-- oxc_span::Span
//! Used by:
//! rust/crates/
//! |-- codemode/src/lib.rs
//! |-- codemode/src/parse/expr.rs
//! `-- codemode/src/run.rs
//! Invariants: no oxc lifetimes escape this module; parse failures are `ParseError` diagnostics.
//! Contract tests: codemode/tests/codemode.rs

mod expr;

use crate::ast::{
    CatchClause, ForEach, ForInit, ForLeft, FunctionBody, FunctionNode, Loc, Pattern,
    PatternProperty,
};
use crate::ast::{Stmt, SwitchCase, VarDecl, VarKind};
use oxc_allocator::Allocator;
use oxc_ast::ast;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;
use std::sync::Arc;

/// Parses `code` as the body of an implicit async function.
pub(crate) fn parse_program(code: &str) -> Result<Vec<Stmt>, String> {
    let allocator = Allocator::default();
    let source_type = SourceType::ts().with_module(true);
    let options = ParseOptions {
        allow_return_outside_function: true,
        preserve_parens: false,
        ..ParseOptions::default()
    };
    let parsed = Parser::new(&allocator, code, source_type)
        .with_options(options)
        .parse();
    if let Some(diagnostic) = parsed.diagnostics.iter().next() {
        return Err(format!(
            "Failed to parse TypeScript: {}",
            diagnostic.message
        ));
    }
    if parsed.fatal_error {
        return Err("Failed to parse script as a Program node.".into());
    }
    let lower = Lower::new(code);
    let mut body: Vec<Stmt> = parsed
        .program
        .directives
        .iter()
        .map(|directive| {
            let loc = lower.loc(directive.span);
            Stmt::Expr(crate::ast::Expr::Literal(
                crate::ast::Literal::Str(directive.expression.value.to_string()),
                loc,
            ))
        })
        .collect();
    for statement in &parsed.program.body {
        body.push(lower.statement(statement)?);
    }
    Ok(body)
}

pub(super) struct Lower {
    line_starts: Vec<usize>,
    source: String,
}

impl Lower {
    fn new(source: &str) -> Self {
        let mut line_starts = vec![0];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        Self {
            line_starts,
            source: source.to_string(),
        }
    }

    pub(super) fn loc(&self, span: oxc_span::Span) -> Loc {
        let offset = span.start as usize;
        let line = self.line_starts.partition_point(|start| *start <= offset);
        let start = self.line_starts[line.saturating_sub(1)];
        let column = self
            .source
            .get(start..offset)
            .map_or(0, |text| text.encode_utf16().count());
        Loc {
            line: line as u32,
            column: column as u32 + 1,
        }
    }

    fn statement(&self, statement: &ast::Statement<'_>) -> Result<Stmt, String> {
        use ast::Statement as S;
        Ok(match statement {
            S::ExpressionStatement(node) => Stmt::Expr(self.expr(&node.expression)?),
            S::VariableDeclaration(node) => Stmt::Var(self.var_decl(node)?),
            S::ReturnStatement(node) => Stmt::Return(
                node.argument
                    .as_ref()
                    .map(|value| self.expr(value))
                    .transpose()?,
            ),
            S::BlockStatement(node) => Stmt::Block(self.statements(&node.body)?),
            S::IfStatement(node) => Stmt::If {
                test: self.expr(&node.test)?,
                consequent: Box::new(self.statement(&node.consequent)?),
                alternate: node
                    .alternate
                    .as_ref()
                    .map(|item| self.statement(item).map(Box::new))
                    .transpose()?,
            },
            S::SwitchStatement(node) => Stmt::Switch {
                discriminant: self.expr(&node.discriminant)?,
                cases: node
                    .cases
                    .iter()
                    .map(|case| {
                        Ok(SwitchCase {
                            test: case.test.as_ref().map(|test| self.expr(test)).transpose()?,
                            consequent: self.statements(&case.consequent)?,
                        })
                    })
                    .collect::<Result<_, String>>()?,
                loc: self.loc(node.span),
            },
            S::WhileStatement(node) => Stmt::While {
                test: self.expr(&node.test)?,
                body: Box::new(self.statement(&node.body)?),
            },
            S::DoWhileStatement(node) => Stmt::DoWhile {
                body: Box::new(self.statement(&node.body)?),
                test: self.expr(&node.test)?,
            },
            S::ForStatement(node) => Stmt::For {
                init: match &node.init {
                    None => None,
                    Some(ast::ForStatementInit::VariableDeclaration(decl)) => {
                        Some(ForInit::Var(self.var_decl(decl)?))
                    }
                    Some(other) => Some(ForInit::Expr(self.expr(other.to_expression())?)),
                },
                test: node.test.as_ref().map(|test| self.expr(test)).transpose()?,
                update: node
                    .update
                    .as_ref()
                    .map(|update| self.expr(update))
                    .transpose()?,
                body: Box::new(self.statement(&node.body)?),
            },
            S::ForOfStatement(node) => Stmt::ForOf(Box::new(ForEach {
                left: self.for_left(&node.left)?,
                right: self.expr(&node.right)?,
                body: self.statement(&node.body)?,
                is_await: node.r#await,
                loc: self.loc(node.span),
            })),
            S::ForInStatement(node) => Stmt::ForIn(Box::new(ForEach {
                left: self.for_left(&node.left)?,
                right: self.expr(&node.right)?,
                body: self.statement(&node.body)?,
                is_await: false,
                loc: self.loc(node.span),
            })),
            S::BreakStatement(node) => Stmt::Break {
                labeled: node.label.is_some(),
                loc: self.loc(node.span),
            },
            S::ContinueStatement(node) => Stmt::Continue {
                labeled: node.label.is_some(),
                loc: self.loc(node.span),
            },
            S::ThrowStatement(node) => Stmt::Throw(self.expr(&node.argument)?),
            S::TryStatement(node) => Stmt::Try {
                block: Box::new(Stmt::Block(self.statements(&node.block.body)?)),
                handler: node
                    .handler
                    .as_ref()
                    .map(|handler| {
                        Ok::<_, String>(CatchClause {
                            param: handler
                                .param
                                .as_ref()
                                .map(|param| self.pattern(&param.pattern))
                                .transpose()?,
                            body: Box::new(Stmt::Block(self.statements(&handler.body.body)?)),
                        })
                    })
                    .transpose()?,
                finalizer: node
                    .finalizer
                    .as_ref()
                    .map(|block| {
                        Ok::<_, String>(Box::new(Stmt::Block(self.statements(&block.body)?)))
                    })
                    .transpose()?,
            },
            S::EmptyStatement(_) => Stmt::Empty,
            S::FunctionDeclaration(node) => match (&node.id, &node.body) {
                (Some(id), Some(_)) => Stmt::Function {
                    name: id.name.to_string(),
                    function: self.function(node)?,
                    loc: self.loc(node.span),
                },
                // Overload signatures and `declare function` are type-only.
                _ => Stmt::Empty,
            },
            S::TSTypeAliasDeclaration(_)
            | S::TSInterfaceDeclaration(_)
            | S::TSGlobalDeclaration(_)
            | S::TSExternalModuleDeclaration(_) => Stmt::Empty,
            S::TSNamespaceDeclaration(node) if node.declare => Stmt::Empty,
            S::ImportDeclaration(_)
            | S::ExportAllDeclaration(_)
            | S::ExportDefaultDeclaration(_)
            | S::ExportNamedDeclaration(_)
            | S::ExportDeclaration(_)
            | S::ExportFromDeclaration(_) => {
                return Err("Failed to parse TypeScript: 'import' and 'export' may only appear in a module; CodeMode programs are scripts.".into());
            }
            other => Stmt::Unsupported {
                kind: statement_kind(other).into(),
                loc: self.loc(oxc_span::GetSpan::span(other)),
            },
        })
    }

    fn statements(&self, statements: &[ast::Statement<'_>]) -> Result<Vec<Stmt>, String> {
        statements
            .iter()
            .map(|statement| self.statement(statement))
            .collect()
    }

    fn var_decl(&self, node: &ast::VariableDeclaration<'_>) -> Result<VarDecl, String> {
        let kind = match node.kind {
            ast::VariableDeclarationKind::Var => VarKind::Var,
            ast::VariableDeclarationKind::Let => VarKind::Let,
            ast::VariableDeclarationKind::Const => VarKind::Const,
            _ => {
                return Err(
                    "Failed to parse TypeScript: 'using' declarations are not supported.".into(),
                );
            }
        };
        Ok(VarDecl {
            kind,
            declarations: node
                .declarations
                .iter()
                .map(|declarator| {
                    Ok((
                        self.pattern(&declarator.id)?,
                        declarator
                            .init
                            .as_ref()
                            .map(|init| self.expr(init))
                            .transpose()?,
                        self.loc(declarator.span),
                    ))
                })
                .collect::<Result<_, String>>()?,
        })
    }

    fn for_left(&self, left: &ast::ForStatementLeft<'_>) -> Result<ForLeft, String> {
        Ok(match left {
            ast::ForStatementLeft::VariableDeclaration(decl) => {
                let lowered = self.var_decl(decl)?;
                let count = lowered.declarations.len();
                let mutable = lowered.kind != VarKind::Const;
                let pattern = lowered.declarations.into_iter().next().map_or(
                    Pattern::Unsupported {
                        kind: "VariableDeclaration".into(),
                        loc: self.loc(decl.span),
                    },
                    |item| item.0,
                );
                ForLeft::Declare {
                    pattern,
                    mutable,
                    count,
                    loc: self.loc(decl.span),
                }
            }
            ast::ForStatementLeft::AssignmentTargetIdentifier(ident) => ForLeft::Assign {
                name: ident.name.to_string(),
                loc: self.loc(ident.span),
            },
            other => ForLeft::Unsupported(self.loc(oxc_span::GetSpan::span(other))),
        })
    }

    pub(super) fn pattern(&self, pattern: &ast::BindingPattern<'_>) -> Result<Pattern, String> {
        Ok(match pattern {
            ast::BindingPattern::BindingIdentifier(ident) => {
                Pattern::Ident(ident.name.to_string(), self.loc(ident.span))
            }
            ast::BindingPattern::AssignmentPattern(node) => Pattern::Default {
                left: Box::new(self.pattern(&node.left)?),
                right: Box::new(self.expr(&node.right)?),
                loc: self.loc(node.span),
            },
            ast::BindingPattern::ObjectPattern(node) => Pattern::Object {
                properties: node
                    .properties
                    .iter()
                    .map(|property| {
                        Ok(PatternProperty {
                            key: if property.computed {
                                None
                            } else {
                                property_key_name(&property.key)
                            },
                            key_loc: self.loc(oxc_span::GetSpan::span(&property.key)),
                            value: self.pattern(&property.value)?,
                            loc: self.loc(property.span),
                        })
                    })
                    .collect::<Result<_, String>>()?,
                rest: node
                    .rest
                    .as_ref()
                    .map(|rest| self.pattern(&rest.argument).map(Box::new))
                    .transpose()?,
                loc: self.loc(node.span),
            },
            ast::BindingPattern::ArrayPattern(node) => Pattern::Array {
                elements: node
                    .elements
                    .iter()
                    .map(|element| element.as_ref().map(|item| self.pattern(item)).transpose())
                    .collect::<Result<_, String>>()?,
                rest: node
                    .rest
                    .as_ref()
                    .map(|rest| self.pattern(&rest.argument).map(Box::new))
                    .transpose()?,
                loc: self.loc(node.span),
            },
        })
    }

    pub(super) fn function(&self, node: &ast::Function<'_>) -> Result<Arc<FunctionNode>, String> {
        let body = match &node.body {
            Some(body) => FunctionBody::Block(Stmt::Block(self.statements(&body.statements)?)),
            None => FunctionBody::Block(Stmt::Block(Vec::new())),
        };
        let (params, rest) = self.params(&node.params)?;
        Ok(Arc::new(FunctionNode {
            params,
            rest,
            body,
            generator: node.generator,
            loc: self.loc(node.span),
        }))
    }

    pub(super) fn params(
        &self,
        params: &ast::FormalParameters<'_>,
    ) -> Result<(Vec<Pattern>, Option<Pattern>), String> {
        let items = params
            .items
            .iter()
            .map(|param| {
                let pattern = self.pattern(&param.pattern)?;
                Ok(match &param.initializer {
                    Some(init) => Pattern::Default {
                        loc: pattern.loc(),
                        left: Box::new(pattern),
                        right: Box::new(self.expr(init)?),
                    },
                    None => pattern,
                })
            })
            .collect::<Result<_, String>>()?;
        let rest = params
            .rest
            .as_ref()
            .map(|rest| self.pattern(&rest.rest.argument))
            .transpose()?;
        Ok((items, rest))
    }
}

pub(super) fn property_key_name(key: &ast::PropertyKey<'_>) -> Option<String> {
    match key {
        ast::PropertyKey::StaticIdentifier(ident) => Some(ident.name.to_string()),
        ast::PropertyKey::StringLiteral(literal) => Some(literal.value.to_string()),
        ast::PropertyKey::NumericLiteral(literal) => {
            Some(crate::value::number::format(literal.value))
        }
        _ => None,
    }
}

fn statement_kind(statement: &ast::Statement<'_>) -> &'static str {
    use ast::Statement as S;
    match statement {
        S::DebuggerStatement(_) => "DebuggerStatement",
        S::LabeledStatement(_) => "LabeledStatement",
        S::WithStatement(_) => "WithStatement",
        S::ClassDeclaration(_) => "ClassDeclaration",
        S::TSEnumDeclaration(_) => "TSEnumDeclaration",
        S::TSNamespaceDeclaration(_) => "TSModuleDeclaration",
        S::TSImportEqualsDeclaration(_) => "TSImportEqualsDeclaration",
        _ => "Statement",
    }
}
