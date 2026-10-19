//! Purpose: home, session and plugin navigation with explicit back history.
//! Scope: application lifetime.
//! Uses:
//! `-- none
//! External:
//! `-- none
//! Used by:
//! rust/crates/
//! `-- ui-terminal/src/app.rs
//! Invariants: repeated navigation does not duplicate history; back never invents sessions.
//! Contract tests: ui-terminal/tests/application.rs

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Route {
    #[default]
    Home,
    Session(String),
    Plugin(String),
}

#[derive(Debug, Default)]
pub struct Navigation {
    pub current: Route,
    history: Vec<Route>,
}

impl Navigation {
    pub fn navigate(&mut self, route: Route) {
        if self.current == route {
            return;
        }
        self.history
            .push(std::mem::replace(&mut self.current, route));
    }

    pub fn back(&mut self) {
        self.current = self.history.pop().unwrap_or_default();
    }

    pub fn forget(&mut self, id: &str) {
        self.history
            .retain(|route| route != &Route::Session(id.into()));
        if self.current == Route::Session(id.into()) {
            self.back();
        }
    }
}
