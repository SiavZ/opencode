//! Purpose: pure path, URI, language-id and UTF-16 position helpers shared by the LSP client
//! and server inventory. Ports node `path.extname`/`path.relative` semantics used by
//! packages/opencode/src/lsp/{client,lsp,language}.ts.
//! Scope: synchronous, no I/O.
//! Uses:
//! `-- none
//! External:
//! `-- url::Url
//! Used by:
//! rust/crates/
//! |-- integration-adapters/src/lsp/client.rs
//! |-- integration-adapters/src/lsp/diagnostics.rs
//! |-- integration-adapters/src/lsp/mod.rs
//! |-- integration-adapters/src/lsp/requests.rs
//! |-- integration-adapters/src/lsp/roots.rs
//! `-- integration-adapters/src/lsp/service.rs
//! Invariants: `end_position` counts UTF-16 code units and splits on `\r\n|\r|\n` like the baseline.
//! Contract tests: integration-adapters/src/lsp/text.rs (tests module)

use std::path::{Component, Path, PathBuf};

use url::Url;

/// `LANGUAGE_EXTENSIONS` (packages/opencode/src/lsp/language.ts:1-121), baseline order.
pub const LANGUAGE_EXTENSIONS: &[(&str, &str)] = &[
    (".abap", "abap"),
    (".bat", "bat"),
    (".bib", "bibtex"),
    (".bibtex", "bibtex"),
    (".clj", "clojure"),
    (".cljs", "clojure"),
    (".cljc", "clojure"),
    (".edn", "clojure"),
    (".coffee", "coffeescript"),
    (".c", "c"),
    (".cpp", "cpp"),
    (".cxx", "cpp"),
    (".cc", "cpp"),
    (".c++", "cpp"),
    (".cs", "csharp"),
    (".csx", "csharp"),
    (".css", "css"),
    (".d", "d"),
    (".pas", "pascal"),
    (".pascal", "pascal"),
    (".diff", "diff"),
    (".patch", "diff"),
    (".dart", "dart"),
    (".dockerfile", "dockerfile"),
    (".ex", "elixir"),
    (".exs", "elixir"),
    (".erl", "erlang"),
    (".ets", "typescript"),
    (".hrl", "erlang"),
    (".fs", "fsharp"),
    (".fsi", "fsharp"),
    (".fsx", "fsharp"),
    (".fsscript", "fsharp"),
    (".gitcommit", "git-commit"),
    (".gitrebase", "git-rebase"),
    (".go", "go"),
    (".groovy", "groovy"),
    (".gleam", "gleam"),
    (".hbs", "handlebars"),
    (".handlebars", "handlebars"),
    (".hs", "haskell"),
    (".lhs", "haskell"),
    (".html", "html"),
    (".htm", "html"),
    (".ini", "ini"),
    (".java", "java"),
    (".jl", "julia"),
    (".js", "javascript"),
    (".kt", "kotlin"),
    (".kts", "kotlin"),
    (".jsx", "javascriptreact"),
    (".json", "json"),
    (".tex", "latex"),
    (".latex", "latex"),
    (".less", "less"),
    (".lua", "lua"),
    (".makefile", "makefile"),
    ("makefile", "makefile"),
    (".md", "markdown"),
    (".markdown", "markdown"),
    (".m", "objective-c"),
    (".mm", "objective-cpp"),
    (".pl", "perl"),
    (".pm", "perl"),
    (".pm6", "perl6"),
    (".php", "php"),
    (".ps1", "powershell"),
    (".psm1", "powershell"),
    (".pug", "jade"),
    (".jade", "jade"),
    (".py", "python"),
    (".r", "r"),
    (".cshtml", "razor"),
    (".razor", "razor"),
    (".rb", "ruby"),
    (".rake", "ruby"),
    (".gemspec", "ruby"),
    (".ru", "ruby"),
    (".erb", "erb"),
    (".html.erb", "erb"),
    (".js.erb", "erb"),
    (".css.erb", "erb"),
    (".json.erb", "erb"),
    (".rs", "rust"),
    (".scss", "scss"),
    (".sass", "sass"),
    (".scala", "scala"),
    (".shader", "shaderlab"),
    (".sh", "shellscript"),
    (".bash", "shellscript"),
    (".zsh", "shellscript"),
    (".ksh", "shellscript"),
    (".sql", "sql"),
    (".svelte", "svelte"),
    (".swift", "swift"),
    (".ts", "typescript"),
    (".tsx", "typescriptreact"),
    (".mts", "typescript"),
    (".cts", "typescript"),
    (".mtsx", "typescriptreact"),
    (".ctsx", "typescriptreact"),
    (".xml", "xml"),
    (".xsl", "xsl"),
    (".yaml", "yaml"),
    (".yml", "yaml"),
    (".mjs", "javascript"),
    (".cjs", "javascript"),
    (".vue", "vue"),
    (".zig", "zig"),
    (".zon", "zig"),
    (".astro", "astro"),
    (".ml", "ocaml"),
    (".mli", "ocaml"),
    (".tf", "terraform"),
    (".tfvars", "terraform-vars"),
    (".hcl", "hcl"),
    (".nix", "nix"),
    (".typ", "typst"),
    (".typc", "typst"),
];

/// Language id for `didOpen`: `LANGUAGE_EXTENSIONS[path.extname(file)] ?? "plaintext"`.
pub fn language_id(file: &str) -> &'static str {
    let ext = extname(file);
    LANGUAGE_EXTENSIONS
        .iter()
        .find(|(k, _)| *k == ext)
        .map(|(_, v)| *v)
        .unwrap_or("plaintext")
}

/// node `path.extname`: last `.` of the basename, empty for dotfiles and names without a dot.
pub fn extname(file: &str) -> &str {
    let base = file
        .rsplit(['/', std::path::MAIN_SEPARATOR])
        .next()
        .unwrap_or(file);
    match base.rfind('.') {
        Some(0) | None => "",
        Some(i) => &base[i..],
    }
}

/// Server matching key in lsp.ts: `path.parse(file).ext || file`.
pub fn match_extension(file: &str) -> &str {
    let ext = extname(file);
    if ext.is_empty() { file } else { ext }
}

/// Resolve `file` against `directory` when relative, then normalize `.`/`..` lexically.
pub fn absolute(directory: &Path, file: &str) -> PathBuf {
    let p = Path::new(file);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        directory.join(p)
    };
    normalize(&joined)
}

pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `FSUtil.contains(parent, child)`.
pub fn contains(parent: &Path, child: &Path) -> bool {
    normalize(child).starts_with(normalize(parent))
}

/// node `path.relative(from, to)` for absolute paths.
pub fn relative(from: &Path, to: &Path) -> String {
    let from: Vec<_> = normalize(from)
        .components()
        .map(|c| c.as_os_str().to_owned())
        .collect();
    let to: Vec<_> = normalize(to)
        .components()
        .map(|c| c.as_os_str().to_owned())
        .collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let parts: Vec<String> = std::iter::repeat_n("..".to_string(), from.len() - common)
        .chain(
            to[common..]
                .iter()
                .map(|s| s.to_string_lossy().into_owned()),
        )
        .collect();
    parts.join(std::path::MAIN_SEPARATOR_STR)
}

/// `pathToFileURL(path).href`.
pub fn file_uri(path: &Path) -> String {
    Url::from_file_path(path)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}

/// `getFilePath(uri)`: only `file://` URIs map to paths.
pub fn uri_to_path(uri: &str) -> Option<String> {
    if !uri.starts_with("file://") {
        return None;
    }
    let url = Url::parse(uri).ok()?;
    url.to_file_path()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// `endPosition(text)`: last line index and its length in UTF-16 code units.
pub fn end_position(text: &str) -> (u32, u32) {
    let mut line = 0u32;
    let mut last_start = 0usize;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                line += 1;
                i += 2;
                last_start = i;
            }
            b'\r' | b'\n' => {
                line += 1;
                i += 1;
                last_start = i;
            }
            _ => i += 1,
        }
    }
    (line, text[last_start..].encode_utf16().count() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extname_matches_node() {
        assert_eq!(extname("/a/b.ts"), ".ts");
        assert_eq!(extname("/a/.bashrc"), "");
        assert_eq!(extname("/a/Dockerfile"), "");
        assert_eq!(extname("x.html.erb"), ".erb");
        assert_eq!(match_extension("Dockerfile"), "Dockerfile");
        assert_eq!(language_id("/a/main.rs"), "rust");
        assert_eq!(language_id("/a/Makefile"), "plaintext");
    }

    #[test]
    fn end_position_counts_utf16() {
        assert_eq!(end_position(""), (0, 0));
        assert_eq!(end_position("a\r\nb\rc\nd😀"), (3, 3));
        assert_eq!(end_position("x\n"), (1, 0));
    }

    #[test]
    fn relative_and_contains() {
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/b/c")), "c");
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/b")), "");
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/x")), "../x");
        assert!(contains(Path::new("/a"), Path::new("/a/b/../c")));
        assert!(!contains(Path::new("/a"), Path::new("/ab")));
        assert_eq!(
            uri_to_path(&file_uri(Path::new("/t/a b.rs"))).unwrap(),
            "/t/a b.rs"
        );
        assert_eq!(uri_to_path("untitled:x"), None);
    }
}
