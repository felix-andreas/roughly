//! Loading declaration-only `.Rtypes` stub files onto interned types.
//!
//! Each line is `name : <type-expr>` — the type half reuses the `#:`
//! annotation grammar (parsed by wrapping it as an annotation and running the
//! ordinary pipeline, so there is no second type parser). `@type NAME`
//! declares an opaque stub nominal; repeating a name within one source
//! appends an ordered overload candidate; a later source replaces a name's
//! whole set. The assembled library is derived from a set-once singleton
//! input, so stub text never participates in per-edit invalidation.

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
    /// Variadic functions whose `...` arguments are data-masked.
    pub masked: FxHashSet<String>,
}

/// The shipped stdlib corpus (base + default-attached packages), embedded so
/// the data survives independent of the legacy crate tree.
pub fn shipped_stub_sources() -> Vec<(String, String)> {
    [
        (
            "base",
            include_str!("../../analysis-legacy/stubs/base.Rtypes"),
        ),
        (
            "stats",
            include_str!("../../analysis-legacy/stubs/stats.Rtypes"),
        ),
        (
            "utils",
            include_str!("../../analysis-legacy/stubs/utils.Rtypes"),
        ),
        (
            "methods",
            include_str!("../../analysis-legacy/stubs/methods.Rtypes"),
        ),
        (
            "graphics",
            include_str!("../../analysis-legacy/stubs/graphics.Rtypes"),
        ),
        (
            "grDevices",
            include_str!("../../analysis-legacy/stubs/grDevices.Rtypes"),
        ),
    ]
    .into_iter()
    .map(|(namespace, text)| (namespace.to_owned(), text.to_owned()))
    .collect()
}

/// Parse and lower every stub source into the interned library.
#[salsa::tracked(returns(ref))]
pub fn stub_library<'db>(db: &'db dyn Db, sources: StubSources) -> StubLibrary<'db> {
    let mut library = StubLibrary::default();
    for (_namespace, text) in sources.sources(db) {
        // Names declared earlier in THIS source append candidates; a name
        // first seen in this source replaces any earlier source's set.
        let mut seen_here: FxHashSet<&str> = FxHashSet::default();
        for raw_line in text.lines() {
            let content = strip_comment(raw_line).trim();
            if content.is_empty() {
                continue;
            }
            if let Some(rest) = content.strip_prefix("@type") {
                let name = rest.trim();
                if !name.is_empty() {
                    library.nominals.insert(name.to_owned());
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
            if let Some(rest) = type_text.strip_prefix("@masked") {
                type_text = rest.trim_start();
                library.masked.insert(name.to_owned());
            }
            let Some(scheme) = lower_type_text(db, type_text) else {
                continue;
            };
            if seen_here.insert(name) {
                library.schemes.insert(name.to_owned(), vec![scheme]);
            } else if let Some(candidates) = library.schemes.get_mut(name) {
                candidates.push(scheme);
            }
        }
    }
    library
}

/// The library assembled from the singleton input (`None` when unset).
pub fn stubs<'db>(db: &'db dyn Db) -> Option<&'db StubLibrary<'db>> {
    StubSources::try_get(db).map(|sources| stub_library(db, sources))
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
