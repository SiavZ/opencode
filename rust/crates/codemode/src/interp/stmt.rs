//! Purpose: statement evaluation (blocks, control flow, loops, switch, try/catch/finally).
//! Scope: `runtime.ts` evaluateStatement..evaluateVariableDeclaration (lines 773-1360).
//! Uses:
//! rust/crates/
//! |-- codemode/src/ast.rs
//! |-- codemode/src/error.rs
//! |-- codemode/src/interp/diagnose.rs
//! `-- codemode/src/interp/mod.rs
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- codemode/src/interp/mod.rs
//! Invariants: interruption is never caught by try/catch or finally; scopes always unwind.
//! Contract tests: codemode/tests/parity.rs, codemode/tests/enumeration.rs

use super::diagnose::caught_value;
use super::{Completion, Fut, Interp};
use crate::ast::{ForEach, ForInit, ForLeft, Stmt, VarDecl, VarKind};
use crate::error::{DiagnosticKind, Eval, Fault, ValueBox, fail, fail_kind, unsupported_syntax};
use crate::value::{Binding, Value, lock, shared};
use indexmap::IndexMap;

impl Interp {
    pub(super) fn stmt<'a>(&'a mut self, statement: &'a Stmt) -> Fut<'a, Completion> {
        Box::pin(async move {
            self.tick().await?;
            match statement {
                Stmt::Expr(expression) => Ok(Completion::Value(self.expr(expression).await?)),
                Stmt::Var(declaration) => {
                    self.var_decl(declaration).await?;
                    Ok(Completion::None)
                }
                Stmt::Return(argument) => Ok(Completion::Return(match argument {
                    Some(argument) => self.expr(argument).await?,
                    None => Value::Undefined,
                })),
                Stmt::Block(body) => {
                    self.push_scope();
                    let result = self.block(body).await;
                    self.pop_scope();
                    result
                }
                Stmt::If {
                    test,
                    consequent,
                    alternate,
                } => {
                    if self.expr(test).await?.truthy() {
                        self.stmt(consequent).await
                    } else if let Some(alternate) = alternate {
                        self.stmt(alternate).await
                    } else {
                        Ok(Completion::None)
                    }
                }
                Stmt::Switch {
                    discriminant,
                    cases,
                    loc,
                } => {
                    self.push_scope();
                    let result = self.switch(discriminant, cases, *loc).await;
                    self.pop_scope();
                    result
                }
                Stmt::While { test, body } => {
                    while self.expr(test).await?.truthy() {
                        match self.stmt(body).await? {
                            Completion::Break => break,
                            Completion::Return(value) => return Ok(Completion::Return(value)),
                            Completion::Value(value) => self.last_value = value,
                            Completion::Continue | Completion::None => {}
                        }
                    }
                    Ok(Completion::None)
                }
                Stmt::DoWhile { body, test } => {
                    loop {
                        match self.stmt(body).await? {
                            Completion::Break => break,
                            Completion::Return(value) => return Ok(Completion::Return(value)),
                            Completion::Value(value) => self.last_value = value,
                            Completion::Continue | Completion::None => {}
                        }
                        if !self.expr(test).await?.truthy() {
                            break;
                        }
                    }
                    Ok(Completion::None)
                }
                Stmt::For {
                    init,
                    test,
                    update,
                    body,
                } => {
                    self.push_scope();
                    let result = self
                        .for_loop(init.as_ref(), test.as_ref(), update.as_ref(), body)
                        .await;
                    self.pop_scope();
                    result
                }
                Stmt::ForOf(each) => self.for_of(each).await,
                Stmt::ForIn(each) => self.for_in(each).await,
                Stmt::Break { labeled, loc } => {
                    if *labeled {
                        return Err(fail("Labeled break is not supported in v1.", *loc));
                    }
                    Ok(Completion::Break)
                }
                Stmt::Continue { labeled, loc } => {
                    if *labeled {
                        return Err(fail("Labeled continue is not supported in v1.", *loc));
                    }
                    Ok(Completion::Continue)
                }
                Stmt::Throw(argument) => {
                    let value = self.expr(argument).await?;
                    Err(Fault::Throw(ValueBox(value)))
                }
                Stmt::Try {
                    block,
                    handler,
                    finalizer,
                } => {
                    self.try_stmt(block, handler.as_ref(), finalizer.as_deref())
                        .await
                }
                Stmt::Empty | Stmt::Function { .. } => Ok(Completion::None),
                Stmt::Unsupported { kind, loc } => Err(unsupported_syntax(kind, *loc)),
            }
        })
    }

    pub(super) async fn block(&mut self, body: &[Stmt]) -> Eval<Completion> {
        self.hoist(body)?;
        for statement in body {
            match self.stmt(statement).await? {
                Completion::Value(value) => self.last_value = value,
                Completion::None => {}
                other => return Ok(other),
            }
        }
        Ok(Completion::None)
    }

    async fn switch(
        &mut self,
        discriminant: &crate::ast::Expr,
        cases: &[crate::ast::SwitchCase],
        loc: crate::ast::Loc,
    ) -> Eval<Completion> {
        let value = self.expr(discriminant).await?;
        if super::ops::contains_opaque(&value) {
            return Err(fail_kind(
                "Switch discriminants must be data values in CodeMode.",
                loc,
                DiagnosticKind::InvalidDataValue,
            ));
        }
        let mut default_index = None;
        let mut selected = None;
        for (index, case) in cases.iter().enumerate() {
            let Some(test) = &case.test else {
                default_index = Some(index);
                continue;
            };
            let candidate = self.expr(test).await?;
            if super::ops::contains_opaque(&candidate) {
                return Err(fail_kind(
                    "Switch case values must be data values in CodeMode.",
                    test.loc(),
                    DiagnosticKind::InvalidDataValue,
                ));
            }
            if candidate.strict_equals(&value) {
                selected = Some(index);
                break;
            }
        }
        let Some(start) = selected.or(default_index) else {
            return Ok(Completion::None);
        };
        for case in &cases[start..] {
            for statement in &case.consequent {
                match self.stmt(statement).await? {
                    Completion::Break => return Ok(Completion::None),
                    Completion::Value(value) => self.last_value = value,
                    Completion::None => {}
                    other => return Ok(other),
                }
            }
        }
        Ok(Completion::None)
    }

    async fn for_loop(
        &mut self,
        init: Option<&ForInit>,
        test: Option<&crate::ast::Expr>,
        update: Option<&crate::ast::Expr>,
        body: &Stmt,
    ) -> Eval<Completion> {
        let mut per_iteration: Vec<String> = Vec::new();
        match init {
            Some(ForInit::Var(declaration)) => {
                self.var_decl(declaration).await?;
                if declaration.kind != VarKind::Var {
                    per_iteration = lock(&self.current_scope()).keys().cloned().collect();
                }
            }
            Some(ForInit::Expr(expression)) => {
                self.expr(expression).await?;
            }
            None => {}
        }
        loop {
            if let Some(test) = test
                && !self.expr(test).await?.truthy()
            {
                break;
            }
            let iteration = (!per_iteration.is_empty()).then(|| {
                let loop_scope = lock(&self.current_scope()).clone();
                let copies: IndexMap<String, Binding> = per_iteration
                    .iter()
                    .filter_map(|name| {
                        loop_scope
                            .get(name)
                            .map(|binding| (name.clone(), binding.clone()))
                    })
                    .collect();
                shared(copies)
            });
            if let Some(scope) = &iteration {
                self.scopes.push(scope.clone());
            }
            let result = self.stmt(body).await;
            if iteration.is_some() {
                self.pop_scope();
            }
            let completion = result?;
            match completion {
                Completion::Return(value) => return Ok(Completion::Return(value)),
                Completion::Break => break,
                Completion::Value(value) => self.last_value = value,
                Completion::Continue | Completion::None => {}
            }
            if let Some(scope) = &iteration {
                let copies = lock(scope).clone();
                let loop_scope = self.current_scope();
                let mut loop_scope = lock(&loop_scope);
                for name in &per_iteration {
                    if let Some(binding) = copies.get(name) {
                        loop_scope.insert(name.clone(), binding.clone());
                    }
                }
            }
            if let Some(update) = update {
                self.expr(update).await?;
            }
        }
        Ok(Completion::None)
    }

    async fn for_of(&mut self, each: &ForEach) -> Eval<Completion> {
        if each.is_await {
            return Err(fail("for await...of is not supported.", each.loc));
        }
        let right = self.expr(&each.right).await?;
        let Some(items) = super::collections::spread_items(&right) else {
            return Err(fail(
                "for...of requires an array, string, Map, or Set value in CodeMode.",
                each.loc,
            ));
        };
        self.iterate(each, items, "for...of").await
    }

    async fn for_in(&mut self, each: &ForEach) -> Eval<Completion> {
        let right = self.expr(&each.right).await?;
        let Some(keys) = self.enumerable_keys(&right)? else {
            return Err(fail(
                "for...in requires a plain object, array, or tools reference in CodeMode. Use for...of for arrays/strings/Maps/Sets, or Object.keys(value) for a key list.",
                each.loc,
            ));
        };
        self.iterate(each, keys.into_iter().map(Value::str).collect(), "for...in")
            .await
    }

    async fn iterate(
        &mut self,
        each: &ForEach,
        items: Vec<Value>,
        label: &str,
    ) -> Eval<Completion> {
        if let ForLeft::Declare { count, loc, .. } = &each.left
            && *count != 1
        {
            return Err(fail(
                format!("{label} supports one declared binding."),
                *loc,
            ));
        }
        if let ForLeft::Unsupported(loc) = &each.left {
            return Err(fail(format!("Unsupported {label} binding."), *loc));
        }
        for item in items {
            let declared = match &each.left {
                ForLeft::Declare {
                    pattern,
                    mutable,
                    loc,
                    ..
                } => {
                    self.push_scope();
                    if let Err(fault) = self.bind_pattern(pattern, item, *mutable, *loc).await {
                        self.pop_scope();
                        return Err(fault);
                    }
                    true
                }
                ForLeft::Assign { name, loc } => {
                    self.set_identifier(name, item, *loc)?;
                    false
                }
                ForLeft::Unsupported(_) => false,
            };
            let result = self.stmt(&each.body).await;
            if declared {
                self.pop_scope();
            }
            match result? {
                Completion::Return(value) => return Ok(Completion::Return(value)),
                Completion::Break => return Ok(Completion::None),
                Completion::Value(value) => self.last_value = value,
                Completion::Continue | Completion::None => {}
            }
        }
        Ok(Completion::None)
    }

    async fn try_stmt(
        &mut self,
        block: &Stmt,
        handler: Option<&crate::ast::CatchClause>,
        finalizer: Option<&Stmt>,
    ) -> Eval<Completion> {
        let mut attempted = self.stmt(block).await;
        if let (Err(fault), Some(handler)) = (&attempted, handler)
            && !matches!(fault, Fault::Interrupted)
        {
            let caught = caught_value(fault);
            self.push_scope();
            let result = match &handler.param {
                Some(param) => match self.bind_pattern(param, caught, true, param.loc()).await {
                    Ok(()) => self.stmt(&handler.body).await,
                    Err(fault) => Err(fault),
                },
                None => self.stmt(&handler.body).await,
            };
            self.pop_scope();
            attempted = result;
        }
        let Some(finalizer) = finalizer else {
            return attempted;
        };
        if matches!(attempted, Err(Fault::Interrupted)) {
            return attempted;
        }
        let finished = self.stmt(finalizer).await?;
        if matches!(
            finished,
            Completion::Return(_) | Completion::Break | Completion::Continue
        ) {
            return Ok(finished);
        }
        attempted
    }

    pub(super) async fn var_decl(&mut self, declaration: &VarDecl) -> Eval<()> {
        for (pattern, init, loc) in &declaration.declarations {
            let value = match init {
                Some(init) => self.expr(init).await?,
                None => Value::Undefined,
            };
            self.bind_pattern(pattern, value, declaration.kind != VarKind::Const, *loc)
                .await?;
        }
        Ok(())
    }
}
