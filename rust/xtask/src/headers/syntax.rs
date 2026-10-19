//! Purpose: collect Rust module and import references, including inline module scopes.
//! Scope: syntax analysis for dependency headers.
//! Uses:
//! `-- none
//! External:
//! |-- char::is_lowercase
//! |-- syn::ItemMod
//! |-- syn::ItemUse
//! |-- syn::Path
//! |-- syn::UseTree
//! |-- syn::UseTree::Group
//! |-- syn::UseTree::Name
//! |-- syn::UseTree::Path
//! |-- syn::UseTree::Rename
//! |-- syn::visit::Visit
//! |-- syn::visit::visit_item_mod
//! `-- syn::visit::visit_path
//! Used by:
//! rust/xtask/
//! `-- src/headers.rs
//! Invariants: grouped and renamed imports retain their original referenced paths.
//! Contract tests: xtask/tests/gates.rs

use syn::visit::Visit;

#[derive(Default)]
pub(super) struct References {
    pub uses: Vec<Vec<String>>,
    pub modules: Vec<(Vec<String>, syn::ItemMod)>,
    scope: Vec<String>,
}

impl<'ast> Visit<'ast> for References {
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        flatten(&item.tree, Vec::new(), &self.scope, &mut self.uses);
    }
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if item.content.is_none() {
            self.modules.push((self.scope.clone(), item.clone()));
            return;
        }
        self.scope.push(item.ident.to_string());
        syn::visit::visit_item_mod(self, item);
        self.scope.pop();
    }
    fn visit_path(&mut self, path: &'ast syn::Path) {
        let parts = path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>();
        if parts.len() > 1
            && parts
                .first()
                .is_some_and(|p| p.chars().next().is_some_and(char::is_lowercase))
        {
            let mut scoped = self.scope.clone();
            scoped.push("@".into());
            scoped.extend(parts);
            self.uses.push(scoped);
        }
        syn::visit::visit_path(self, path);
    }
}

fn flatten(tree: &syn::UseTree, prefix: Vec<String>, scope: &[String], out: &mut Vec<Vec<String>>) {
    match tree {
        syn::UseTree::Path(path) => {
            let mut next = prefix;
            next.push(path.ident.to_string());
            flatten(&path.tree, next, scope, out);
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                flatten(item, prefix.clone(), scope, out);
            }
        }
        leaf => {
            let mut parts = scope.to_vec();
            parts.push("@".into());
            parts.extend(prefix);
            parts.push(match leaf {
                syn::UseTree::Name(name) => name.ident.to_string(),
                syn::UseTree::Rename(name) => name.ident.to_string(),
                _ => "*".into(),
            });
            out.push(parts);
        }
    }
}
