//! Loading declaration-only `.Rtypes` stub files onto interned types.
//!
//! Each line is `name : <type-expr>` — the type half reuses the `#:`
//! annotation grammar (parsed by wrapping it as an annotation and running the
//! ordinary pipeline, so there is no second type parser). `@type NAME`
//! declares an opaque stub nominal; repeating a name within one source
//! appends an ordered overload candidate; a later source replaces a name's
//! whole set. The assembled library is derived from a set-once singleton
//! input plus the package-metadata input (which activates the conditional
//! namespaces), so stub text never participates in per-edit invalidation.

use crate::Db;
use crate::annotations::lower_annotation;
use crate::types::TypeScheme;
use rustc_hash::{FxHashMap, FxHashSet};

/// The raw stub sources: `(namespace, text)` pairs in precedence order (later
/// sources replace earlier declarations of the same name wholesale).
#[salsa::input(singleton, debug)]
pub struct StubSources {
    #[returns(ref)]
    pub sources: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, salsa::SalsaValue)]
pub struct StubLibrary<'db> {
    /// Ordered overload candidates per name (single-element for plain names).
    pub schemes: FxHashMap<String, Vec<TypeScheme<'db>>>,
    /// Opaque nominal type names (`@type data.frame`).
    pub nominals: FxHashSet<String>,
    /// Variadic functions whose `...` arguments are data-masked, mapped to
    /// the names of the formals declared BEFORE the `...`: arguments
    /// matching those formals (by position or name) resolve normally, and
    /// everything the rest parameter absorbs is masked. An empty list means
    /// every argument masks (`join_by(...)`-style vocabulary).
    pub masked: FxHashMap<String, Vec<String>>,
    /// Declared names per namespace. Declaration-level, not winner-level: a
    /// later source overriding a name's type does not un-export it from the
    /// namespace that declared it.
    pub exports_by_namespace: FxHashMap<String, FxHashSet<String>>,
    /// The winning declaration site per name — hover and goto-definition jump
    /// here. For an overload set this is the first candidate's line.
    pub declarations: FxHashMap<String, StubDeclaration>,
}

/// Where a stub name is declared: which installed source (an index into the
/// `StubSources` order the host fed in — the host knows each index's file)
/// and the name token's range within that source's text.
#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)]
pub struct StubDeclaration {
    pub source_index: usize,
    pub range: syntax::TextRange,
}

/// The winning declaration site of `name`, when the corpus declares it.
pub fn stub_declaration<'db>(db: &'db dyn Db, name: &str) -> Option<&'db StubDeclaration> {
    stubs(db)?.declarations.get(name)
}

/// Whether the stub corpus knows `package` as a namespace. `None` when no
/// corpus is installed — callers skip validation entirely then.
pub fn namespace_known(db: &dyn Db, package: &str) -> Option<bool> {
    stubs(db).map(|library| library.exports_by_namespace.contains_key(package))
}

/// The namespace whose declaration of `name` currently wins: sources are in
/// precedence order (later replaces earlier), so the last declaring namespace
/// owns the name.
pub fn declaring_namespace<'db>(db: &'db dyn Db, name: &str) -> Option<&'db str> {
    let sources = StubSources::try_get(db)?;
    let library = stub_library(db, sources);
    sources.sources(db).iter().rev().find_map(|(namespace, _)| {
        library
            .exports_by_namespace
            .get(namespace)
            .is_some_and(|names| names.contains(name))
            .then_some(namespace.as_str())
    })
}

/// Whether `package` declares `name` (any declaration form, overloads and
/// nominals included).
pub fn namespace_exports(db: &dyn Db, package: &str, name: &str) -> bool {
    stubs(db).is_some_and(|library| {
        library
            .exports_by_namespace
            .get(package)
            .is_some_and(|names| names.contains(name))
    })
}

/// The shipped stdlib corpus (base + default-attached packages, plus the
/// conditional namespaces), embedded from this crate's own `stubs/`
/// directory.
pub fn shipped_stub_sources() -> Vec<(String, String)> {
    [
        ("base", include_str!("../stubs/base.Rtypes")),
        ("stats", include_str!("../stubs/stats.Rtypes")),
        ("utils", include_str!("../stubs/utils.Rtypes")),
        ("methods", include_str!("../stubs/methods.Rtypes")),
        ("graphics", include_str!("../stubs/graphics.Rtypes")),
        ("grDevices", include_str!("../stubs/grDevices.Rtypes")),
        ("data.table", include_str!("../stubs/data.table.Rtypes")),
        ("dplyr", include_str!("../stubs/dplyr.Rtypes")),
    ]
    .into_iter()
    .map(|(namespace, text)| (namespace.to_owned(), text.to_owned()))
    .collect()
}

/// Shipped namespaces R does not attach by default: their declarations join
/// the library only when the project declares or attaches the package
/// (`metadata::namespace_active`), so `fread` and `mutate` never resolve —
/// and never steal a typo warning — in a project that does not use them.
pub const CONDITIONAL_NAMESPACES: &[&str] = &["data.table", "dplyr"];

/// Parse and lower every stub source into the interned library.
#[salsa::tracked(returns(ref))]
pub fn stub_library<'db>(db: &'db dyn Db, sources: StubSources) -> StubLibrary<'db> {
    // A namespace declared by more than one source carries a PROJECT
    // override on top of the shipped file — and writing `stubs/dplyr.Rtypes`
    // is itself the clearest declaration that the project uses the package,
    // so it activates the conditional namespace like metadata would.
    let mut seen_namespaces: FxHashSet<&str> = FxHashSet::default();
    let mut project_overridden: FxHashSet<&str> = FxHashSet::default();
    for (namespace, _) in sources.sources(db) {
        if !seen_namespaces.insert(namespace.as_str()) {
            project_overridden.insert(namespace.as_str());
        }
    }
    let mut library = StubLibrary::default();
    for (source_index, (namespace, text)) in sources.sources(db).iter().enumerate() {
        if CONDITIONAL_NAMESPACES.contains(&namespace.as_str())
            && !project_overridden.contains(namespace.as_str())
            && !crate::metadata::namespace_active(db, namespace)
        {
            continue;
        }
        let namespace_exports = library
            .exports_by_namespace
            .entry(namespace.clone())
            .or_default();
        // Names declared earlier in THIS source append candidates; a name
        // first seen in this source replaces any earlier source's set.
        let mut seen_here: FxHashSet<&str> = FxHashSet::default();
        let mut line_start = 0usize;
        for raw_segment in text.split_inclusive('\n') {
            let raw_line = raw_segment.trim_end_matches(['\n', '\r']);
            let this_line_start = line_start;
            line_start += raw_segment.len();
            let content = strip_comment(raw_line).trim();
            if content.is_empty() {
                continue;
            }
            // The declared name's first occurrence in the uncommented line is
            // the name token itself (only whitespace or `@type` precede it).
            let name_range = |name: &str| {
                strip_comment(raw_line).find(name).map(|at| {
                    let start = (this_line_start + at) as u32;
                    syntax::TextRange::new(start.into(), (start + name.len() as u32).into())
                })
            };
            if let Some(rest) = content.strip_prefix("@type") {
                let name = rest.trim();
                if !name.is_empty() {
                    library.nominals.insert(name.to_owned());
                    namespace_exports.insert(name.to_owned());
                    if let Some(range) = name_range(name) {
                        library.declarations.insert(
                            name.to_owned(),
                            StubDeclaration {
                                source_index,
                                range,
                            },
                        );
                    }
                }
                continue;
            }
            let Some(separator) = top_level_colon(content) else {
                continue;
            };
            let name = content[..separator].trim();
            let mut type_text = content[separator + 1..].trim();
            if name.is_empty() || !is_stub_name(name) {
                continue;
            }
            let masked = if let Some(rest) = type_text.strip_prefix("@masked") {
                type_text = rest.trim_start();
                true
            } else {
                false
            };
            let Some(scheme) = lower_type_text(db, type_text) else {
                continue;
            };
            if masked && let Some(leading) = masked_leading_formals(db, &scheme) {
                library.masked.insert(name.to_owned(), leading);
            }
            namespace_exports.insert(name.to_owned());
            if seen_here.insert(name) {
                library.schemes.insert(name.to_owned(), vec![scheme]);
                if let Some(range) = name_range(name) {
                    library.declarations.insert(
                        name.to_owned(),
                        StubDeclaration {
                            source_index,
                            range,
                        },
                    );
                }
            } else if let Some(candidates) = library.schemes.get_mut(name) {
                candidates.push(scheme);
            }
        }
    }
    library
}

/// The names of a masked declaration's formals before its `...`, in
/// declaration order — the arguments that resolve normally at a masked call
/// (the data arguments). `None` when the scheme is not a variadic function
/// (the declaration-level error `stub_source_problems` reports; the loader
/// simply skips the mask).
fn masked_leading_formals(db: &dyn Db, scheme: &TypeScheme<'_>) -> Option<Vec<String>> {
    let crate::types::TyKind::Function(function) = scheme.body.kind(db) else {
        return None;
    };
    let variadic = function.variadic.as_ref()?;
    let leading = function.named.get(..variadic.preceding_named)?;
    Some(
        leading
            .iter()
            .map(|field| field.name.text(db).to_owned())
            .collect(),
    )
}

/// The library assembled from the singleton input (`None` when unset).
pub fn stubs<'db>(db: &'db dyn Db) -> Option<&'db StubLibrary<'db>> {
    StubSources::try_get(db).map(|sources| stub_library(db, sources))
}

/// One declaration the stub loader would drop: its zero-based line and the
/// reason. The editor's `.Rtypes` buffer diagnostics and `roughly check`'s
/// override report both render from this list, so the wording stays
/// identical everywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubProblem {
    pub line: usize,
    pub message: String,
}

/// Everything the loader would drop from one stub source, ordered by line.
/// The nominal vocabulary is the installed corpus's plus this source's own
/// `@type` declarations — the source may be an unsaved editor buffer whose
/// declarations are not installed yet.
pub fn stub_source_problems(db: &dyn Db, text: &str) -> Vec<StubProblem> {
    let mut known_nominals: FxHashSet<String> = stubs(db)
        .map(|library| library.nominals.iter().cloned().collect())
        .unwrap_or_default();
    for raw_line in text.lines() {
        let content = strip_comment(raw_line).trim();
        if let Some(rest) = content.strip_prefix("@type") {
            let name = rest.trim();
            if !name.is_empty() {
                known_nominals.insert(name.to_owned());
            }
        }
    }

    let mut problems = Vec::new();
    for (line, raw_line) in text.lines().enumerate() {
        let content = strip_comment(raw_line).trim();
        if content.is_empty() {
            continue;
        }
        if let Some(rest) = content.strip_prefix("@type") {
            if rest.trim().is_empty() {
                problems.push(StubProblem {
                    line,
                    message: "expected a type name after `@type`.".to_owned(),
                });
            }
            continue;
        }
        let Some(separator) = top_level_colon(content) else {
            problems.push(StubProblem {
                line,
                message: "this line is not a declaration (`name : TYPE`).".to_owned(),
            });
            continue;
        };
        let name = content[..separator].trim();
        let mut type_text = content[separator + 1..].trim();
        if name.is_empty() || !is_stub_name(name) {
            problems.push(StubProblem {
                line,
                message: format!("`{name}` is not a valid declaration name."),
            });
            continue;
        }
        if type_text.is_empty() {
            problems.push(StubProblem {
                line,
                message: format!("expected a type after `{name} :`."),
            });
            continue;
        }
        let mut masked = false;
        if let Some(rest) = type_text.strip_prefix("@masked") {
            type_text = rest.trim_start();
            masked = true;
        }
        let Some(scheme) = lower_type_text(db, type_text) else {
            problems.push(StubProblem {
                line,
                message: format!(
                    "this declaration does not load: `{type_text}` is not a valid type."
                ),
            });
            continue;
        };
        if masked
            && !matches!(
                scheme.body.kind(db),
                crate::types::TyKind::Function(function) if function.variadic.is_some()
            )
        {
            problems.push(StubProblem {
                line,
                message: format!(
                    "`@masked` on `{name}` requires a variadic function type — the mask covers \
                     the arguments the `...` rest parameter absorbs."
                ),
            });
            continue;
        }
        let mut unknown = Vec::new();
        collect_unknown_nominals(db, scheme.body, &known_nominals, &mut unknown);
        for unknown_name in unknown {
            problems.push(StubProblem {
                line,
                message: format!(
                    "this declaration does not load: I do not know the type `{unknown_name}`."
                ),
            });
        }
    }
    problems
}

/// Named types the nominal vocabulary does not declare, in first-occurrence
/// order. Rigid variables (binder-introduced) are not nominals.
fn collect_unknown_nominals<'db>(
    db: &'db dyn Db,
    ty: crate::types::Ty<'db>,
    known: &FxHashSet<String>,
    out: &mut Vec<String>,
) {
    use crate::types::TyKind;
    match ty.kind(db) {
        TyKind::Named(name, arguments) => {
            let text = name.text(db);
            if !known.contains(text) && !out.iter().any(|seen| seen == text) {
                out.push(text.to_owned());
            }
            for argument in arguments {
                collect_unknown_nominals(db, *argument, known, out);
            }
        }
        TyKind::Vector(inner)
        | TyKind::NamedVector(inner)
        | TyKind::List(inner)
        | TyKind::NamedList(inner) => collect_unknown_nominals(db, *inner, known, out),
        TyKind::Tuple(members) => {
            for member in members {
                collect_unknown_nominals(db, *member, known, out);
            }
        }
        TyKind::Record(fields) => {
            for field in fields {
                collect_unknown_nominals(db, field.ty, known, out);
            }
        }
        TyKind::Function(function) => {
            for positional in &function.positional {
                collect_unknown_nominals(db, *positional, known, out);
            }
            for named in &function.named {
                collect_unknown_nominals(db, named.ty, known, out);
            }
            if let Some(rest) = &function.variadic {
                collect_unknown_nominals(db, rest.element, known, out);
            }
            collect_unknown_nominals(db, function.ret, known, out);
        }
        TyKind::Union(members) => {
            for member in members {
                collect_unknown_nominals(db, *member, known, out);
            }
        }
        TyKind::Any
        | TyKind::Unknown
        | TyKind::Null
        | TyKind::Scalar(_)
        | TyKind::Var(_)
        | TyKind::Rigid(_) => {}
    }
}

/// Parse one type expression by routing it through the annotation pipeline —
/// the single type grammar in the system.
fn lower_type_text<'db>(db: &'db dyn Db, type_text: &str) -> Option<TypeScheme<'db>> {
    let parse = syntax::parse(&format!("#: {type_text}"));
    if !parse.errors().is_empty() {
        return None;
    }
    let root = parse.syntax_node();
    let annotation = root
        .children()
        .find(|node| node.kind() == syntax::SyntaxKind::ANNOTATION)?;
    lower_annotation(db, &annotation).declared
}

fn strip_comment(line: &str) -> &str {
    // `#` cannot occur inside a declaration type, so a line comment starts at
    // the first `#`.
    match line.find('#') {
        Some(at) => &line[..at],
        None => line,
    }
}

/// The first `:` at delimiter depth zero separates name from type (`list[a: b]`
/// keeps its interior colon).
fn top_level_colon(content: &str) -> Option<usize> {
    let mut depth: usize = 0;
    for (byte_index, character) in content.char_indices() {
        match character {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => return Some(byte_index),
            _ => {}
        }
    }
    None
}

/// An R identifier: letters, digits, `.`, `_`, not starting with a digit.
fn is_stub_name(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if first.is_ascii_digit() {
        return false;
    }
    name.chars()
        .all(|c| c.is_alphanumeric() || c == '.' || c == '_')
}

/// Convenience for hosts and tests: feed the shipped corpus into the database.
pub fn install_shipped_stubs(db: &dyn Db) -> StubSources {
    StubSources::new(db, shipped_stub_sources())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::file_diagnostics;
    use crate::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};

    #[test]
    fn shipped_corpus_loads() {
        let db = RootDatabase::default();
        let sources = install_shipped_stubs(&db);
        let library = stub_library(&db, sources);
        assert!(
            library.schemes.len() > 400,
            "expected the full shipped corpus, got {} names",
            library.schemes.len()
        );
        assert!(library.nominals.contains("data.frame"));
        assert_eq!(
            library.schemes["sum"].len(),
            3,
            "sum keeps its ordered overload candidates"
        );
        assert_eq!(library.schemes["length"].len(), 1);
    }

    #[test]
    fn declarations_record_winning_sites() {
        let db = RootDatabase::default();
        let sources = install_shipped_stubs(&db);
        let library = stub_library(&db, sources);
        let declaration = &library.declarations["print"];
        let (namespace, text) = &sources.sources(&db)[declaration.source_index];
        assert_eq!(namespace, "base");
        let range = usize::from(declaration.range.start())..usize::from(declaration.range.end());
        assert_eq!(&text[range], "print");
        let nominal = &library.declarations["data.frame"];
        let (_, nominal_text) = &sources.sources(&db)[nominal.source_index];
        let range = usize::from(nominal.range.start())..usize::from(nominal.range.end());
        assert_eq!(&nominal_text[range], "data.frame");
    }

    #[test]
    fn conditional_namespace_is_dark_without_activation() {
        let db = RootDatabase::default();
        let sources = install_shipped_stubs(&db);
        let library = stub_library(&db, sources);
        assert!(!library.schemes.contains_key("fread"));
        assert!(!library.nominals.contains("data.table"));
        assert!(!library.exports_by_namespace.contains_key("data.table"));
    }

    #[test]
    fn declared_dependency_activates_a_conditional_namespace() {
        let db = RootDatabase::default();
        let sources = install_shipped_stubs(&db);
        crate::metadata::PackageMetadata::new(
            &db,
            Vec::new(),
            ["data.table".to_owned()].into(),
            Default::default(),
        );
        let library = stub_library(&db, sources);
        assert!(library.schemes.contains_key("fread"));
        assert!(library.nominals.contains("data.table"));
    }

    #[test]
    fn library_call_attaches_a_conditional_namespace() {
        let mut db = RootDatabase::default();
        let sources = install_shipped_stubs(&db);
        let metadata = crate::metadata::PackageMetadata::new(
            &db,
            Vec::new(),
            Default::default(),
            Default::default(),
        );
        let file = SourceFile::new(
            &db,
            "library(data.table)\nprint(fread(\"x.csv\"))\n".to_owned(),
            DocumentKind::Script,
        );
        ProjectFiles::new(&db, vec![file]);
        let attached = crate::metadata::attached_union(&db, [file]);
        assert_eq!(
            attached.iter().collect::<Vec<_>>(),
            ["data.table"],
            "the library() call must be scanned"
        );
        use salsa::Setter;
        metadata.set_attached(&mut db).to(attached);
        let library = stub_library(&db, sources);
        assert!(library.schemes.contains_key("fread"));
        let diagnostics = file_diagnostics(&db, file);
        assert!(
            diagnostics.is_empty(),
            "an attached conditional namespace resolves its exports: {diagnostics:?}"
        );
    }

    #[test]
    fn stub_names_resolve_in_package_files() {
        let db = RootDatabase::default();
        install_shipped_stubs(&db);
        let file = SourceFile::new(
            &db,
            "total <- function(x) sum(x, na.rm = TRUE)\nsize <- function(x) length(x)\n".to_owned(),
            DocumentKind::Package,
        );
        ProjectFiles::new(&db, vec![file]);
        let diagnostics = file_diagnostics(&db, file);
        assert!(
            diagnostics.is_empty(),
            "stub names must resolve cleanly: {diagnostics:?}"
        );
    }

    #[test]
    fn stub_signature_mismatch_reports() {
        let db = RootDatabase::default();
        install_shipped_stubs(&db);
        let file = SourceFile::new(
            &db,
            "find_x <- function(x) grepl(1L, x)\n".to_owned(),
            DocumentKind::Package,
        );
        ProjectFiles::new(&db, vec![file]);
        let diagnostics = file_diagnostics(&db, file);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "type-mismatch" && d.message.contains("character")),
            "expected a mismatch against grepl's character pattern: {diagnostics:?}"
        );
    }

    #[test]
    fn nominal_names_count_as_resolvable() {
        let db = RootDatabase::default();
        install_shipped_stubs(&db);
        assert!(crate::package_scheme_exists(&db, "data.frame"));
        assert!(crate::package_scheme_exists(&db, "sum"));
        assert!(!crate::package_scheme_exists(&db, "definitely_not_a_name"));
    }

    fn first_item_check<'db>(db: &'db RootDatabase, source: &str) -> crate::check::ItemCheck<'db> {
        let file = SourceFile::new(db, source.to_owned(), DocumentKind::Package);
        ProjectFiles::new(db, vec![file]);
        let item = crate::item_tree(db, file)[0];
        crate::item_check(db, item).expect("item check")
    }

    fn scheme_return<'db>(
        db: &'db RootDatabase,
        check: &crate::check::ItemCheck<'db>,
    ) -> crate::types::Ty<'db> {
        let scheme = check.scheme.clone().expect("definition scheme");
        let crate::types::TyKind::Function(function) = scheme.body.kind(db).clone() else {
            panic!("expected a function scheme");
        };
        function.ret
    }

    #[test]
    fn overload_selection_picks_the_specific_candidate() {
        let db = RootDatabase::default();
        install_shipped_stubs(&db);
        let check = first_item_check(&db, "f <- function() cumsum(1L)\n");
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        let ret = scheme_return(&db, &check);
        let crate::types::TyKind::Vector(element) = ret.kind(&db) else {
            panic!("expected integer[], got {:?}", ret.kind(&db));
        };
        assert!(matches!(
            element.kind(&db),
            crate::types::TyKind::Scalar(crate::types::Atomic::Integer)
        ));
    }

    #[test]
    fn strict_round_keeps_doubles_double() {
        let db = RootDatabase::default();
        install_shipped_stubs(&db);
        // `sum(1, 2)` is a double at runtime: the strict round must refuse the
        // integer candidate and land on the double one.
        let check = first_item_check(&db, "f <- function() sum(1, 2)\n");
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        assert!(matches!(
            scheme_return(&db, &check).kind(&db),
            crate::types::TyKind::Scalar(crate::types::Atomic::Double)
        ));
    }

    #[test]
    fn courtesy_round_admits_whole_double_literals() {
        let db = RootDatabase::default();
        StubSources::new(
            &db,
            vec![(
                "test".to_owned(),
                "f : fn(x: integer) -> integer\nf : fn(x: integer[]) -> integer[]\n".to_owned(),
            )],
        );
        // No candidate takes a double, but `1` is a whole number: the
        // courtesy round admits it and exact declaration order still decides.
        let check = first_item_check(&db, "g <- function() f(1)\n");
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        assert!(matches!(
            scheme_return(&db, &check).kind(&db),
            crate::types::TyKind::Scalar(crate::types::Atomic::Integer)
        ));
    }

    #[test]
    fn no_matching_overload_reports_with_first_failure() {
        let db = RootDatabase::default();
        StubSources::new(
            &db,
            vec![(
                "test".to_owned(),
                "f : fn(x: integer) -> integer\nf : fn(x: double) -> double\n".to_owned(),
            )],
        );
        let check = first_item_check(&db, "g <- function() f(\"a\")\n");
        assert!(
            check.errors.iter().any(|error| matches!(
                &error.kind,
                crate::check::TypeErrorKind::NoMatchingOverload {
                    name,
                    candidates: 2,
                    first: Some(_),
                } if name == "f"
            )),
            "expected a no-matching-overload report, got {:?}",
            check.errors
        );
    }

    #[test]
    fn unresolved_argument_falls_back_to_the_general_candidate() {
        let db = RootDatabase::default();
        install_shipped_stubs(&db);
        // `x` is a free parameter: selection would let the first candidate
        // greedily pin it, so the call must use the final (most general)
        // declaration and leave `x` generic.
        let check = first_item_check(&db, "f <- function(x) sum(x)\n");
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        let scheme = check.scheme.clone().expect("scheme");
        assert_eq!(
            scheme.binders.len(),
            1,
            "the parameter must stay generic: {scheme:?}"
        );
        assert!(matches!(
            scheme_return(&db, &check).kind(&db),
            crate::types::TyKind::Any
        ));
    }

    #[test]
    fn courtesy_applies_outside_overload_sets_too() {
        let db = RootDatabase::default();
        StubSources::new(
            &db,
            vec![(
                "test".to_owned(),
                "g : fn(n: integer) -> integer[]\n".to_owned(),
            )],
        );
        let ok = first_item_check(&db, "f <- function() g(3)\n");
        assert!(ok.errors.is_empty(), "{:?}", ok.errors);
        let db2 = RootDatabase::default();
        StubSources::new(
            &db2,
            vec![(
                "test".to_owned(),
                "g : fn(n: integer) -> integer[]\n".to_owned(),
            )],
        );
        let bad = first_item_check(&db2, "f <- function() g(2.5)\n");
        assert!(
            bad.errors
                .iter()
                .any(|error| matches!(error.kind, crate::check::TypeErrorKind::Mismatch { .. })),
            "a fractional double must not pass as integer: {:?}",
            bad.errors
        );
    }

    #[test]
    fn namespace_access_resolves_through_globals() {
        let db = RootDatabase::default();
        install_shipped_stubs(&db);
        let check = first_item_check(&db, "f <- function(x) base::length(x)\n");
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        assert!(matches!(
            scheme_return(&db, &check).kind(&db),
            crate::types::TyKind::Scalar(crate::types::Atomic::Integer)
        ));
    }

    #[test]
    fn overloads_and_replacement_across_sources() {
        let db = RootDatabase::default();
        let sources = StubSources::new(
            &db,
            vec![
                (
                    "first".to_owned(),
                    "f : fn(x: integer) -> integer\nf : fn(x: Any) -> Any\n".to_owned(),
                ),
                (
                    "second".to_owned(),
                    "f : fn(x: double) -> double\n".to_owned(),
                ),
            ],
        );
        let library = stub_library(&db, sources);
        let candidates = &library.schemes["f"];
        assert_eq!(candidates.len(), 1, "a later source replaces the whole set");
    }
}
