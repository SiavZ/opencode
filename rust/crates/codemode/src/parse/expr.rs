//! Purpose: lower oxc expressions into the owned CodeMode AST.
//! Scope: expressions, arguments, members, assignment targets. TypeScript-only wrappers
//! (`as`, `satisfies`, `!`, `<T>x`) are erased.
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! `-- codemode/src/parse/mod.rs
//! External:
//! |-- ast::Argument
//! |-- ast::Argument::SpreadElement
//! |-- ast::ArrayExpressionElement::Elision
//! |-- ast::ArrayExpressionElement::SpreadElement
//! |-- ast::ArrowFunctionBody::FunctionBody
//! |-- ast::AssignmentTarget
//! |-- ast::CallExpression
//! |-- ast::ChainElement::CallExpression
//! |-- ast::ChainElement::TSNonNullExpression
//! |-- ast::Expression
//! |-- ast::MemberExpression
//! |-- ast::MemberExpression::ComputedMemberExpression
//! |-- ast::MemberExpression::PrivateFieldExpression
//! |-- ast::MemberExpression::StaticMemberExpression
//! |-- ast::ObjectPropertyKind
//! |-- ast::ObjectPropertyKind::ObjectProperty
//! |-- ast::ObjectPropertyKind::SpreadProperty
//! |-- ast::PropertyKey::NumericLiteral
//! |-- ast::PropertyKey::StaticIdentifier
//! |-- ast::PropertyKey::StringLiteral
//! |-- ast::PropertyKind::Init
//! |-- ast::SimpleAssignmentTarget
//! |-- ast::TemplateLiteral
//! |-- oxc_ast::ast
//! |-- oxc_span::GetSpan
//! `-- oxc_syntax::operator::UpdateOperator::Increment
//! Used by:
//! rust/crates/
//! `-- codemode/src/parse/mod.rs
//! Invariants: unsupported expression kinds lower to `Expr::Unsupported` with an ESTree-style name.
//! Contract tests: codemode/tests/codemode.rs

use super::{Lower, property_key_name};
use crate::ast::{
    Arg, AssignTarget, Expr, FunctionBody, FunctionNode, Literal, Member, MemberProp, ObjectProp,
    PropKey,
};
use oxc_ast::ast;
use oxc_span::GetSpan;
use std::sync::Arc;

impl Lower {
    pub(super) fn expr(&self, expression: &ast::Expression<'_>) -> Result<Expr, String> {
        use ast::Expression as E;
        let loc = self.loc(expression.span());
        Ok(match expression {
            E::BooleanLiteral(node) => Expr::Literal(Literal::Bool(node.value), loc),
            E::NullLiteral(_) => Expr::Literal(Literal::Null, loc),
            E::NumericLiteral(node) => Expr::Literal(Literal::Number(node.value), loc),
            E::StringLiteral(node) => Expr::Literal(Literal::Str(node.value.to_string()), loc),
            E::BigIntLiteral(_) => Expr::BigInt(loc),
            E::RegExpLiteral(node) => Expr::Regex {
                pattern: node.regex.pattern.text.to_string(),
                flags: node.regex.flags.to_string(),
                loc,
            },
            E::TemplateLiteral(node) => self.template(node)?,
            E::Identifier(node) => Expr::Ident(node.name.to_string(), loc),
            E::ArrayExpression(node) => Expr::Array(
                node.elements
                    .iter()
                    .map(|element| match element {
                        ast::ArrayExpressionElement::Elision(_) => Ok(None),
                        ast::ArrayExpressionElement::SpreadElement(spread) => Ok(Some(
                            Arg::Spread(self.expr(&spread.argument)?, self.loc(spread.span)),
                        )),
                        other => Ok(Some(Arg::Expr(self.expr(other.to_expression())?))),
                    })
                    .collect::<Result<_, String>>()?,
                loc,
            ),
            E::ObjectExpression(node) => Expr::Object(
                node.properties
                    .iter()
                    .map(|property| self.object_property(property))
                    .collect::<Result<_, String>>()?,
                loc,
            ),
            E::ArrowFunctionExpression(node) => {
                let (params, rest) = self.params(&node.params)?;
                let body = match &node.body {
                    ast::ArrowFunctionBody::FunctionBody(body) => {
                        FunctionBody::Block(crate::ast::Stmt::Block(
                            body.statements
                                .iter()
                                .map(|item| self.statement(item))
                                .collect::<Result<_, String>>()?,
                        ))
                    }
                    other => FunctionBody::Expr(self.expr(other.to_expression())?),
                };
                Expr::Function(Arc::new(FunctionNode {
                    params,
                    rest,
                    body,
                    generator: false,
                    loc,
                }))
            }
            E::FunctionExpression(node) => Expr::Function(self.function(node)?),
            E::AssignmentExpression(node) => Expr::Assign {
                op: node.operator.as_str(),
                target: self.assign_target(&node.left),
                right: Box::new(self.expr(&node.right)?),
                loc,
            },
            E::AwaitExpression(node) => Expr::Await(Box::new(self.expr(&node.argument)?), loc),
            E::BinaryExpression(node) => Expr::Binary {
                op: node.operator.as_str(),
                left: Box::new(self.expr(&node.left)?),
                right: Box::new(self.expr(&node.right)?),
                loc,
            },
            E::LogicalExpression(node) => Expr::Logical {
                op: node.operator.as_str(),
                left: Box::new(self.expr(&node.left)?),
                right: Box::new(self.expr(&node.right)?),
                loc,
            },
            E::UnaryExpression(node) => Expr::Unary {
                op: node.operator.as_str(),
                argument: Box::new(self.expr(&node.argument)?),
                loc,
            },
            E::UpdateExpression(node) => Expr::Update {
                increment: node.operator == oxc_syntax::operator::UpdateOperator::Increment,
                prefix: node.prefix,
                target: self.simple_target(&node.argument),
                loc,
            },
            E::CallExpression(node) => self.call(node)?,
            E::NewExpression(node) => Expr::New {
                callee: Box::new(self.expr(&node.callee)?),
                args: self.arguments(&node.arguments)?,
                loc,
            },
            E::ChainExpression(node) => Expr::Chain(Box::new(match &node.expression {
                ast::ChainElement::CallExpression(call) => self.call(call)?,
                ast::ChainElement::TSNonNullExpression(inner) => self.expr(&inner.expression)?,
                other => match other.as_member_expression() {
                    Some(member) => Expr::Member(Box::new(self.member(member)?)),
                    None => Expr::Unsupported {
                        kind: "ChainExpression".into(),
                        loc,
                    },
                },
            })),
            E::ConditionalExpression(node) => Expr::Conditional {
                test: Box::new(self.expr(&node.test)?),
                consequent: Box::new(self.expr(&node.consequent)?),
                alternate: Box::new(self.expr(&node.alternate)?),
            },
            E::ParenthesizedExpression(node) => self.expr(&node.expression)?,
            E::TSAsExpression(node) => self.expr(&node.expression)?,
            E::TSSatisfiesExpression(node) => self.expr(&node.expression)?,
            E::TSTypeAssertion(node) => self.expr(&node.expression)?,
            E::TSNonNullExpression(node) => self.expr(&node.expression)?,
            E::TSInstantiationExpression(node) => self.expr(&node.expression)?,
            E::StaticMemberExpression(_)
            | E::ComputedMemberExpression(_)
            | E::PrivateFieldExpression(_) => match expression.as_member_expression() {
                Some(member) => Expr::Member(Box::new(self.member(member)?)),
                None => Expr::Unsupported {
                    kind: "MemberExpression".into(),
                    loc,
                },
            },
            other => Expr::Unsupported {
                kind: expression_kind(other).into(),
                loc,
            },
        })
    }

    fn template(&self, node: &ast::TemplateLiteral<'_>) -> Result<Expr, String> {
        Ok(Expr::Template {
            quasis: node
                .quasis
                .iter()
                .map(|quasi| quasi.value.cooked.as_ref().map(|text| text.to_string()))
                .collect(),
            expressions: node
                .expressions
                .iter()
                .map(|item| self.expr(item))
                .collect::<Result<_, String>>()?,
            loc: self.loc(node.span),
        })
    }

    fn call(&self, node: &ast::CallExpression<'_>) -> Result<Expr, String> {
        Ok(Expr::Call {
            callee: Box::new(self.expr(&node.callee)?),
            args: self.arguments(&node.arguments)?,
            optional: node.optional,
            loc: self.loc(node.span),
        })
    }

    fn arguments(&self, arguments: &[ast::Argument<'_>]) -> Result<Vec<Arg>, String> {
        arguments
            .iter()
            .map(|argument| match argument {
                ast::Argument::SpreadElement(spread) => Ok(Arg::Spread(
                    self.expr(&spread.argument)?,
                    self.loc(spread.span),
                )),
                other => Ok(Arg::Expr(self.expr(other.to_expression())?)),
            })
            .collect()
    }

    fn object_property(
        &self,
        property: &ast::ObjectPropertyKind<'_>,
    ) -> Result<ObjectProp, String> {
        Ok(match property {
            ast::ObjectPropertyKind::SpreadProperty(spread) => {
                ObjectProp::Spread(self.expr(&spread.argument)?, self.loc(spread.span))
            }
            ast::ObjectPropertyKind::ObjectProperty(node) => {
                if node.kind != ast::PropertyKind::Init {
                    return Ok(ObjectProp::Accessor(self.loc(node.span)));
                }
                let key_loc = self.loc(node.key.span());
                let key = if node.computed {
                    match node.key.as_expression() {
                        Some(expression) => PropKey::Computed(self.expr(expression)?),
                        None => PropKey::Ident(String::new()),
                    }
                } else {
                    match &node.key {
                        ast::PropertyKey::StaticIdentifier(ident) => {
                            PropKey::Ident(ident.name.to_string())
                        }
                        ast::PropertyKey::StringLiteral(literal) => {
                            PropKey::Literal(Literal::Str(literal.value.to_string()))
                        }
                        ast::PropertyKey::NumericLiteral(literal) => {
                            PropKey::Literal(Literal::Number(literal.value))
                        }
                        other => match property_key_name(other) {
                            Some(name) => PropKey::Ident(name),
                            None => return Ok(ObjectProp::Accessor(key_loc)),
                        },
                    }
                };
                ObjectProp::Init {
                    key,
                    key_loc,
                    value: self.expr(&node.value)?,
                }
            }
        })
    }

    pub(super) fn member(&self, member: &ast::MemberExpression<'_>) -> Result<Member, String> {
        let loc = self.loc(member.span());
        Ok(match member {
            ast::MemberExpression::StaticMemberExpression(node) => Member {
                object: self.expr(&node.object)?,
                property: MemberProp::Static(
                    node.property.name.to_string(),
                    self.loc(node.property.span),
                ),
                optional: node.optional,
                loc,
            },
            ast::MemberExpression::ComputedMemberExpression(node) => Member {
                object: self.expr(&node.object)?,
                property: MemberProp::Computed(
                    Box::new(self.expr(&node.expression)?),
                    self.loc(node.expression.span()),
                ),
                optional: node.optional,
                loc,
            },
            ast::MemberExpression::PrivateFieldExpression(node) => Member {
                object: self.expr(&node.object)?,
                property: MemberProp::Static(
                    format!("#{}", node.field.name),
                    self.loc(node.field.span),
                ),
                optional: node.optional,
                loc,
            },
        })
    }

    fn assign_target(&self, target: &ast::AssignmentTarget<'_>) -> AssignTarget {
        match target.as_simple_assignment_target() {
            Some(simple) => self.simple_target(simple),
            None => AssignTarget::Invalid(self.loc(target.span())),
        }
    }

    fn simple_target(&self, target: &ast::SimpleAssignmentTarget<'_>) -> AssignTarget {
        use ast::SimpleAssignmentTarget as T;
        match target {
            T::AssignmentTargetIdentifier(ident) => {
                AssignTarget::Ident(ident.name.to_string(), self.loc(ident.span))
            }
            T::TSAsExpression(node) => self.expression_target(&node.expression),
            T::TSSatisfiesExpression(node) => self.expression_target(&node.expression),
            T::TSNonNullExpression(node) => self.expression_target(&node.expression),
            T::TSTypeAssertion(node) => self.expression_target(&node.expression),
            other => match other
                .as_member_expression()
                .map(|member| self.member(member))
            {
                Some(Ok(member)) => AssignTarget::Member(Box::new(member)),
                _ => AssignTarget::Invalid(self.loc(other.span())),
            },
        }
    }

    fn expression_target(&self, expression: &ast::Expression<'_>) -> AssignTarget {
        match self.expr(expression) {
            Ok(Expr::Ident(name, loc)) => AssignTarget::Ident(name, loc),
            Ok(Expr::Member(member)) => AssignTarget::Member(member),
            _ => AssignTarget::Invalid(self.loc(expression.span())),
        }
    }
}

fn expression_kind(expression: &ast::Expression<'_>) -> &'static str {
    use ast::Expression as E;
    match expression {
        E::Super(_) => "Super",
        E::ClassExpression(_) => "ClassExpression",
        E::ImportExpression(_) => "ImportExpression",
        E::SequenceExpression(_) => "SequenceExpression",
        E::TaggedTemplateExpression(_) => "TaggedTemplateExpression",
        E::ThisExpression(_) => "ThisExpression",
        E::YieldExpression(_) => "YieldExpression",
        E::PrivateInExpression(_) => "PrivateInExpression",
        E::ImportMeta(_) | E::NewTarget(_) => "MetaProperty",
        E::JSXElement(_) => "JSXElement",
        E::JSXFragment(_) => "JSXFragment",
        _ => "Expression",
    }
}
