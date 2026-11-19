//! Every function in a crate's production source, with its lines and cyclomatic complexity.
//!
//! The walk starts at `lib.rs` and follows `mod` declarations, so a file reached only through `#[cfg(test)]` is test
//! code and a file no declaration reaches is not part of the crate. Each function records whether the measured
//! platform builds it: a function that isn't built there has no coverage to read and is reported as excluded, never as
//! uncovered.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, Expr, ImplItem, Item, Meta};

/// The platform a coverage report was measured on. `cfg` predicates evaluate against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    Windows,
}

impl Platform {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "linux" => Some(Self::Linux),
            "windows" => Some(Self::Windows),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }

    fn target_os(self) -> &'static str {
        self.name()
    }

    fn family(self) -> &'static str {
        match self {
            Self::Linux => "unix",
            Self::Windows => "windows",
        }
    }
}

/// Why a function has no coverage to measure on this platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Excluded {
    /// `#[cfg(...)]` on the function or an enclosing item is false here; the text is the predicate.
    NotBuilt(String),
}

impl fmt::Display for Excluded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBuilt(cfg) => write!(f, "not built on this platform: cfg({cfg})"),
        }
    }
}

/// One function or method in production source.
#[derive(Clone, Debug)]
pub struct Function {
    /// `path/to/file.rs::module::Type::name`, unique within the crate (a repeat gets `#2`, `#3`, ...).
    pub key: String,
    /// The file, relative to the source root, with `/` separators.
    pub file: String,
    pub name: String,
    pub start: usize,
    pub end: usize,
    /// Lines inside `start..=end` that belong to a nested function item instead of this one.
    pub nested: Vec<(usize, usize)>,
    pub complexity: u32,
    pub excluded: Option<Excluded>,
}

impl Function {
    /// Whether `line` is one of this function's own lines.
    pub fn owns(&self, line: usize) -> bool {
        (self.start..=self.end).contains(&line) && !self.nested.iter().any(|&(s, e)| (s..=e).contains(&line))
    }
}

#[derive(Debug)]
pub enum SourceError {
    Read(PathBuf, std::io::Error),
    Parse(PathBuf, syn::Error),
    /// A `mod name;` whose file exists in neither place Rust looks.
    MissingModule(PathBuf, String),
    /// A `cfg` predicate this reader can't evaluate. It fails rather than guess whether the code is built.
    UnknownCfg(PathBuf, usize, String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(p, e) => write!(f, "read {}: {e}", p.display()),
            Self::Parse(p, e) => {
                let at = e.span().start();
                write!(f, "parse {}:{}:{}: {e}", p.display(), at.line, at.column + 1)
            }
            Self::MissingModule(p, m) => write!(f, "{}: no file for `mod {m};`", p.display()),
            Self::UnknownCfg(p, line, c) => write!(f, "{}:{line}: can't evaluate cfg({c})", p.display()),
        }
    }
}

impl std::error::Error for SourceError {}

/// Lists every production function under `root` (the folder holding `lib.rs`), as built on `platform`.
pub fn functions(root: &Path, platform: Platform) -> Result<Vec<Function>, SourceError> {
    let mut walk = Walk {
        root: root.to_path_buf(),
        platform,
        out: Vec::new(),
    };
    walk.file(&root.join("lib.rs"), root, &[], None)?;
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for f in &mut walk.out {
        let n = seen.entry(f.key.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            f.key = format!("{}#{n}", f.key);
        }
    }
    Ok(walk.out)
}

struct Walk {
    root: PathBuf,
    platform: Platform,
    out: Vec<Function>,
}

struct Scope<'a> {
    file: &'a Path,
    rel: String,
    path: Vec<String>,
}

impl Walk {
    /// `dir` is where this file's child modules live; `excluded` is inherited from an enclosing `cfg`.
    fn file(
        &mut self,
        path: &Path,
        dir: &Path,
        modules: &[String],
        excluded: Option<Excluded>,
    ) -> Result<(), SourceError> {
        let text = std::fs::read_to_string(path).map_err(|e| SourceError::Read(path.to_path_buf(), e))?;
        let parsed = syn::parse_file(&text).map_err(|e| SourceError::Parse(path.to_path_buf(), e))?;
        let rel = path
            .strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let scope = Scope {
            file: path,
            rel,
            path: modules.to_vec(),
        };
        if self.is_test(path, &parsed.attrs)? {
            return Ok(());
        }
        self.items(&parsed.items, &scope, dir, excluded)
    }

    fn items(
        &mut self,
        items: &[Item],
        scope: &Scope,
        dir: &Path,
        excluded: Option<Excluded>,
    ) -> Result<(), SourceError> {
        for item in items {
            match item {
                Item::Fn(f) => {
                    let Some(ex) = self.gate(scope.file, &f.attrs, &excluded)? else {
                        continue;
                    };
                    self.push(scope, None, &f.sig.ident.to_string(), f.span(), &f.block, ex);
                }
                Item::Impl(imp) => {
                    let Some(ex) = self.gate(scope.file, &imp.attrs, &excluded)? else {
                        continue;
                    };
                    let ty = type_name(&imp.self_ty);
                    let owner = match &imp.trait_ {
                        Some((_, t, _)) => format!("<{ty} as {}>", path_name(t)),
                        None => ty,
                    };
                    for ii in &imp.items {
                        if let ImplItem::Fn(m) = ii {
                            let Some(ex) = self.gate(scope.file, &m.attrs, &ex)? else {
                                continue;
                            };
                            self.push(scope, Some(&owner), &m.sig.ident.to_string(), m.span(), &m.block, ex);
                        }
                    }
                }
                Item::Trait(t) => {
                    let Some(ex) = self.gate(scope.file, &t.attrs, &excluded)? else {
                        continue;
                    };
                    for ti in &t.items {
                        if let syn::TraitItem::Fn(m) = ti
                            && let Some(body) = &m.default
                        {
                            let Some(ex) = self.gate(scope.file, &m.attrs, &ex)? else {
                                continue;
                            };
                            self.push(
                                scope,
                                Some(&t.ident.to_string()),
                                &m.sig.ident.to_string(),
                                m.span(),
                                body,
                                ex,
                            );
                        }
                    }
                }
                Item::Mod(m) => {
                    let Some(ex) = self.gate(scope.file, &m.attrs, &excluded)? else {
                        continue;
                    };
                    let name = m.ident.to_string();
                    let mut modules = scope.path.clone();
                    modules.push(name.clone());
                    let child_dir = dir.join(&name);
                    match &m.content {
                        Some((_, inner)) => {
                            let inner_scope = Scope {
                                file: scope.file,
                                rel: scope.rel.clone(),
                                path: modules,
                            };
                            self.items(inner, &inner_scope, &child_dir, ex)?;
                        }
                        None => {
                            let flat = dir.join(format!("{name}.rs"));
                            let nested = dir.join(&name).join("mod.rs");
                            let (file, next_dir) = if flat.is_file() {
                                (flat, child_dir)
                            } else if nested.is_file() {
                                (nested, child_dir)
                            } else {
                                return Err(SourceError::MissingModule(scope.file.to_path_buf(), name));
                            };
                            self.file(&file, &next_dir, &modules, ex)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// `None` when the item is test code (skip it entirely); otherwise its exclusion, if the platform doesn't build it.
    fn gate(
        &self,
        file: &Path,
        attrs: &[Attribute],
        inherited: &Option<Excluded>,
    ) -> Result<Option<Option<Excluded>>, SourceError> {
        if self.is_test(file, attrs)? {
            return Ok(None);
        }
        if inherited.is_some() {
            return Ok(Some(inherited.clone()));
        }
        for attr in attrs.iter().filter(|a| a.path().is_ident("cfg")) {
            let Meta::List(list) = &attr.meta else { continue };
            let pred: Meta = list
                .parse_args()
                .map_err(|e| SourceError::Parse(file.to_path_buf(), e))?;
            if !self.eval(file, &pred, false)? {
                return Ok(Some(Some(Excluded::NotBuilt(tidy(&list.tokens.to_string())))));
            }
        }
        Ok(Some(None))
    }

    /// True when a `cfg` on these attributes is false outside tests but true under `cfg(test)`: test-only code.
    /// `#[test]` functions count too.
    fn is_test(&self, file: &Path, attrs: &[Attribute]) -> Result<bool, SourceError> {
        if attrs.iter().any(|a| a.path().is_ident("test")) {
            return Ok(true);
        }
        for attr in attrs.iter().filter(|a| a.path().is_ident("cfg")) {
            let Meta::List(list) = &attr.meta else { continue };
            let pred: Meta = list
                .parse_args()
                .map_err(|e| SourceError::Parse(file.to_path_buf(), e))?;
            let production = [self.eval(file, &pred, false)?, self.eval_any_platform(file, &pred)?];
            if !production.contains(&true) && self.eval(file, &pred, true)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether `pred` holds on either measured platform outside tests: false means only `cfg(test)` can build it.
    fn eval_any_platform(&self, file: &Path, pred: &Meta) -> Result<bool, SourceError> {
        for p in [Platform::Linux, Platform::Windows] {
            if (Walk {
                root: self.root.clone(),
                platform: p,
                out: Vec::new(),
            })
            .eval(file, pred, false)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn eval(&self, file: &Path, pred: &Meta, test: bool) -> Result<bool, SourceError> {
        let unknown = |m: &Meta| {
            let line = m.span().start().line;
            SourceError::UnknownCfg(file.to_path_buf(), line, quote::quote!(#m).to_string())
        };
        match pred {
            Meta::Path(p) => match p.get_ident().map(|i| i.to_string()).as_deref() {
                Some("test") => Ok(test),
                Some("unix") => Ok(self.platform.family() == "unix"),
                Some("windows") => Ok(self.platform.family() == "windows"),
                _ => Err(unknown(pred)),
            },
            Meta::NameValue(nv) => {
                let value = match &nv.value {
                    Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s), ..
                    }) => s.value(),
                    _ => return Err(unknown(pred)),
                };
                match nv.path.get_ident().map(|i| i.to_string()).as_deref() {
                    Some("target_os") => Ok(self.platform.target_os() == value),
                    Some("target_family") => Ok(self.platform.family() == value),
                    _ => Err(unknown(pred)),
                }
            }
            Meta::List(list) => {
                let args = list
                    .parse_args_with(syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated)
                    .map_err(|e| SourceError::Parse(file.to_path_buf(), e))?;
                let values = args
                    .iter()
                    .map(|m| self.eval(file, m, test))
                    .collect::<Result<Vec<_>, _>>()?;
                match list.path.get_ident().map(|i| i.to_string()).as_deref() {
                    Some("any") => Ok(values.contains(&true)),
                    Some("all") => Ok(!values.contains(&false)),
                    Some("not") if values.len() == 1 => Ok(!values[0]),
                    _ => Err(unknown(pred)),
                }
            }
        }
    }

    fn push(
        &mut self,
        scope: &Scope,
        owner: Option<&str>,
        name: &str,
        span: Span,
        body: &syn::Block,
        excluded: Option<Excluded>,
    ) {
        let mut counter = Complexity {
            count: 1,
            ..Complexity::default()
        };
        counter.visit_block(body);
        let mut parts = vec![scope.rel.clone()];
        parts.extend(scope.path.iter().cloned());
        if let Some(o) = owner {
            parts.push(o.to_string());
        }
        parts.push(name.to_string());
        self.out.push(Function {
            key: parts.join("::"),
            file: scope.rel.clone(),
            name: owner.map_or_else(|| name.to_string(), |o| format!("{o}::{name}")),
            start: span.start().line,
            end: span.end().line,
            nested: counter.nested.clone(),
            complexity: counter.count,
            excluded,
        });
        // Nested function items are functions of their own, measured separately.
        let nested_items = counter.items;
        for f in nested_items {
            let Ok(Some(ex)) = self.gate(scope.file, &f.attrs, &None) else {
                continue;
            };
            let inner = Scope {
                file: scope.file,
                rel: scope.rel.clone(),
                path: [scope.path.clone(), vec![name.to_string()]].concat(),
            };
            self.push(&inner, None, &f.sig.ident.to_string(), f.span(), &f.block, ex);
        }
    }
}

/// Cyclomatic complexity: 1, plus one per decision. A decision is an `if`, `while` or `for`, each `match` arm after
/// the first, each match guard, each `&&` and `||`, each `?`, and each `let ... else`. Closures count toward the
/// function that holds them, and so do the methods of an impl written inside a function body. A nested n item
/// is a function of its own.
#[derive(Default)]
struct Complexity {
    count: u32,
    nested: Vec<(usize, usize)>,
    items: Vec<syn::ItemFn>,
}

impl Complexity {
    fn add(&mut self, n: usize) {
        self.count += u32::try_from(n).unwrap_or(u32::MAX);
    }
}

impl<'ast> Visit<'ast> for Complexity {
    fn visit_expr_if(&mut self, e: &'ast syn::ExprIf) {
        self.add(1);
        visit::visit_expr_if(self, e);
    }
    fn visit_expr_while(&mut self, e: &'ast syn::ExprWhile) {
        self.add(1);
        visit::visit_expr_while(self, e);
    }
    fn visit_expr_for_loop(&mut self, e: &'ast syn::ExprForLoop) {
        self.add(1);
        visit::visit_expr_for_loop(self, e);
    }
    fn visit_expr_match(&mut self, e: &'ast syn::ExprMatch) {
        self.add(e.arms.len().saturating_sub(1));
        self.add(e.arms.iter().filter(|a| a.guard.is_some()).count());
        visit::visit_expr_match(self, e);
    }
    fn visit_expr_binary(&mut self, e: &'ast syn::ExprBinary) {
        if matches!(e.op, syn::BinOp::And(_) | syn::BinOp::Or(_)) {
            self.add(1);
        }
        visit::visit_expr_binary(self, e);
    }
    fn visit_expr_try(&mut self, e: &'ast syn::ExprTry) {
        self.add(1);
        visit::visit_expr_try(self, e);
    }
    fn visit_local(&mut self, l: &'ast syn::Local) {
        if l.init.as_ref().is_some_and(|i| i.diverge.is_some()) {
            self.add(1);
        }
        visit::visit_local(self, l);
    }
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        let span = f.span();
        self.nested.push((span.start().line, span.end().line));
        self.items.push(f.clone());
    }
}

/// `not (any (unix , windows))` as `not(any(unix, windows))`.
fn tidy(tokens: &str) -> String {
    tokens
        .replace(" (", "(")
        .replace("( ", "(")
        .replace(" )", ")")
        .replace(" ,", ",")
}

fn type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => path_name(&p.path),
        syn::Type::Reference(r) => type_name(&r.elem),
        other => quote::quote!(#other).to_string().replace(' ', ""),
    }
}

fn path_name(path: &syn::Path) -> String {
    path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/crate/src")
    }

    fn by_key(platform: Platform) -> BTreeMap<String, Function> {
        functions(&fixture(), platform)
            .unwrap()
            .into_iter()
            .map(|f| (f.key.clone(), f))
            .collect()
    }

    #[test]
    fn lists_every_production_function_and_none_of_the_tests() {
        let keys: Vec<String> = by_key(Platform::Linux).into_keys().collect();
        assert_eq!(
            keys,
            [
                "deep.rs::deep::Named::name",
                "deep/inner.rs::deep::inner::again::same",
                "deep/inner.rs::deep::inner::same",
                "lib.rs::<S as Display>::fmt",
                "lib.rs::S::m",
                "lib.rs::decide",
                "lib.rs::linux_only",
                "lib.rs::outer",
                "lib.rs::outer::helper",
                "lib.rs::tests_or_windows",
                "win.rs::win::windows_only",
            ]
        );
    }

    #[test]
    fn counts_each_kind_of_decision_once() {
        let f = by_key(Platform::Linux);
        // 1 + if + && + ? + let-else + || in the closure + 2 more match arms + guard + for.
        assert_eq!(f["lib.rs::decide"].complexity, 10);
        assert_eq!(f["lib.rs::S::m"].complexity, 1);
        assert_eq!(f["deep.rs::deep::Named::name"].complexity, 1);
    }

    #[test]
    fn a_nested_function_is_its_own_function_and_not_its_parents_lines() {
        let f = by_key(Platform::Linux);
        let (outer, helper) = (&f["lib.rs::outer"], &f["lib.rs::outer::helper"]);
        assert_eq!((outer.complexity, helper.complexity), (1, 2));
        assert_eq!((helper.start, helper.end), (28, 30));
        assert!(outer.owns(27) && outer.owns(31));
        assert!(!outer.owns(29), "the helper's body is the helper's");
        assert!(helper.owns(29));
    }

    #[test]
    fn code_the_platform_does_not_build_is_excluded_with_its_cfg() {
        let linux = by_key(Platform::Linux);
        let windows = by_key(Platform::Windows);
        assert_eq!(linux["lib.rs::linux_only"].excluded, None);
        assert_eq!(
            windows["lib.rs::linux_only"].excluded,
            Some(Excluded::NotBuilt("target_os = \"linux\"".into()))
        );
        // A module's cfg reaches every function in its file.
        assert_eq!(
            linux["win.rs::win::windows_only"].excluded,
            Some(Excluded::NotBuilt("windows".into()))
        );
        assert_eq!(windows["win.rs::win::windows_only"].excluded, None);
        // cfg(any(test, windows)) is production code on Windows, so it is listed, and excluded on Linux.
        assert_eq!(windows["lib.rs::tests_or_windows"].excluded, None);
        assert!(linux["lib.rs::tests_or_windows"].excluded.is_some());
    }

    #[test]
    fn repeated_paths_get_numbered_keys() {
        let dir = tempdir("repeat");
        std::fs::write(
            dir.join("lib.rs"),
            "#[cfg(unix)]\npub fn f() {}\n#[cfg(windows)]\npub fn f() {}\n",
        )
        .unwrap();
        let keys: Vec<String> = functions(&dir, Platform::Linux)
            .unwrap()
            .into_iter()
            .map(|f| f.key)
            .collect();
        assert_eq!(keys, ["lib.rs::f", "lib.rs::f#2"]);
    }

    #[test]
    fn an_unknown_cfg_or_missing_module_is_an_error_not_a_guess() {
        let dir = tempdir("unknown");
        std::fs::write(dir.join("lib.rs"), "#[cfg(feature = \"x\")]\npub fn f() {}\n").unwrap();
        let err = functions(&dir, Platform::Linux).unwrap_err().to_string();
        assert!(err.contains("can't evaluate cfg(feature = \"x\")"), "{err}");

        std::fs::write(dir.join("lib.rs"), "mod gone;\n").unwrap();
        let err = functions(&dir, Platform::Linux).unwrap_err().to_string();
        assert!(err.contains("no file for `mod gone;`"), "{err}");

        std::fs::write(dir.join("lib.rs"), "pub fn f( {}\n").unwrap();
        assert!(
            functions(&dir, Platform::Linux)
                .unwrap_err()
                .to_string()
                .starts_with("parse ")
        );
    }

    pub(crate) fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quality-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
