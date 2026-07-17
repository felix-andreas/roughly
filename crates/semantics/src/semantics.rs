//! Semantic analysis over `syntax` trees: the salsa database and all queries.
//!
//! The pipeline is a set of memoized, dependency-tracked queries: parse →
//! per-item item tree → HIR → naming → inference, with interned types and a
//! symbol-granular package interface resolved through salsa fixpoint cycles.
//! Analysis units are *items* (top-level definitions and nested definitions in
//! class-constructor calls and function bodies), never whole files, so an edit
//! recomputes only the items whose derived values actually changed.

use syntax::Parse;

#[salsa::db]
pub trait Db: salsa::Database {}

#[salsa::db]
#[derive(Clone, Default)]
pub struct RootDatabase {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for RootDatabase {}

#[salsa::db]
impl Db for RootDatabase {}

/// Whether a file participates in the package interface or is a standalone
/// script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub enum DocumentKind {
    Package,
    Script,
}

/// One source document: the text is the ground-truth input; everything else is
/// derived.
#[salsa::input(debug)]
pub struct SourceFile {
    #[returns(deref)]
    pub text: String,
    pub kind: DocumentKind,
}

/// The lossless parse of a file. A full from-scratch parse per text revision:
/// parsing is the cheapest stage, and sub-file incrementality happens one
/// level down — untouched items derive equal values and salsa's early cutoff
/// prunes everything downstream.
#[salsa::tracked(returns(clone))]
pub fn parse(db: &dyn Db, file: SourceFile) -> ParseResult {
    ParseResult(syntax::parse(file.text(db)))
}

/// Newtype giving `syntax::Parse` the salsa value plumbing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseResult(pub Parse);

impl std::ops::Deref for ParseResult {
    type Target = Parse;

    fn deref(&self) -> &Parse {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use salsa::Setter as _;

    #[test]
    fn parse_tracks_text_edits() {
        let mut db = RootDatabase::default();
        let file = SourceFile::new(&db, "x <- 1".to_owned(), DocumentKind::Package);
        let first = parse(&db, file);
        assert!(first.errors().is_empty());
        assert_eq!(first.text(), "x <- 1");

        file.set_text(&mut db).to("x <- ".to_owned());
        let second = parse(&db, file);
        assert_eq!(second.text(), "x <- ");
        assert!(!second.errors().is_empty());
    }
}
