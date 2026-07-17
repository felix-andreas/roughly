//! Semantic analysis over `syntax` trees: the salsa database and all queries.
//!
//! The pipeline is a set of memoized, dependency-tracked queries: parse →
//! per-item item tree → HIR → naming → inference, with interned types and a
//! symbol-granular package interface resolved through salsa fixpoint cycles.
//! Analysis units are *items* (top-level definitions and nested definitions in
//! class-constructor calls and function bodies), never whole files, so an edit
//! recomputes only the items whose derived values actually changed.

pub mod hir;

use syntax::Parse;
use syntax::ast::AstNode as _;

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

/// The analysis unit: one top-level definition or statement (nested
/// definitions inside class-constructor calls and function bodies become items
/// in a later slice — the identity scheme already carries `parent` for them).
///
/// Identity is **insertion-stable**: kind + name (+ parent + a disambiguator
/// among same-identity siblings), never a bare position or index — inserting an
/// unrelated item must not shift the identity of items after it, or every
/// downstream memo for them would invalidate.
#[salsa::interned(debug)]
pub struct Item<'db> {
    pub file: SourceFile,
    pub kind: ItemKind,
    #[returns(ref)]
    pub name: Option<String>,
    pub parent: Option<Item<'db>>,
    pub disambiguator: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub enum ItemKind {
    /// `name <- function(...) ...` (any assignment spelling).
    Function,
    /// `name <- <expr>` for a non-function right-hand side.
    Value,
    /// Any other top-level statement (an expression statement, a side-effecting
    /// call, a broken region).
    Statement,
}

/// The ordered items of a file. This is the invalidation barrier between a
/// file's text and per-item work: it carries structure and identity only — no
/// spans, no bodies — so edits inside one body leave it equal and cut off.
#[salsa::tracked(returns(clone))]
pub fn item_tree<'db>(db: &'db dyn Db, file: SourceFile) -> Vec<Item<'db>> {
    let parse = parse(db, file);
    let root = parse.syntax_node();
    let mut counts: rustc_hash::FxHashMap<(ItemKind, Option<String>), u32> =
        rustc_hash::FxHashMap::default();
    let mut items = Vec::new();
    for node in root.children() {
        if !syntax::ast::is_expression_kind(node.kind()) && node.kind() != syntax::SyntaxKind::ERROR
        {
            continue;
        }
        let (kind, name) = classify_top_level(&node);
        let disambiguator = {
            let counter = counts.entry((kind, name.clone())).or_insert(0);
            let current = *counter;
            *counter += 1;
            current
        };
        items.push(Item::new(db, file, kind, name, None, disambiguator));
    }
    items
}

/// The current green subtree of an item (with its item-tree position), or
/// `None` when the item no longer exists in the file.
///
/// The green subtree is width-only and therefore **position-independent**: an
/// edit elsewhere in the file reproduces a structurally equal value here, and
/// salsa's early cutoff prunes all downstream per-item work. Every consumer of
/// item innards must read THIS query (never `parse` directly), and every span
/// it derives must stay item-relative; absolute positions are reintroduced only
/// at the final rendering edge.
#[salsa::tracked(returns(clone))]
pub fn item_syntax<'db>(db: &'db dyn Db, item: Item<'db>) -> Option<ItemSyntax> {
    let parse = parse(db, *item.file(db));
    let root = parse.syntax_node();
    let mut counts: rustc_hash::FxHashMap<(ItemKind, Option<String>), u32> =
        rustc_hash::FxHashMap::default();
    let target = (*item.kind(db), item.name(db).clone(), *item.disambiguator(db));
    for node in root.children() {
        if !syntax::ast::is_expression_kind(node.kind()) && node.kind() != syntax::SyntaxKind::ERROR
        {
            continue;
        }
        let (kind, name) = classify_top_level(&node);
        let counter = counts.entry((kind, name.clone())).or_insert(0);
        let disambiguator = *counter;
        *counter += 1;
        if (kind, name, disambiguator) == target {
            return Some(ItemSyntax(node.green().into()));
        }
    }
    None
}

/// A position-independent green subtree; equality is structural.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemSyntax(pub rowan::GreenNode);

impl ItemSyntax {
    /// A fresh red root over the item's subtree (offsets start at 0 —
    /// item-relative, exactly what per-item consumers must work in).
    pub fn syntax_node(&self) -> syntax::SyntaxNode {
        syntax::SyntaxNode::new_root(self.0.clone())
    }
}

/// The lowered HIR of one item, derived from its position-independent green
/// subtree only — never from the whole file — so it stays equal (and cuts off)
/// across edits elsewhere in the file. `None` when the item no longer exists.
#[salsa::tracked(returns(clone))]
pub fn item_hir<'db>(db: &'db dyn Db, item: Item<'db>) -> Option<hir::Module> {
    let syntax = item_syntax(db, item)?;
    Some(hir::lower_item(&syntax.syntax_node()))
}

/// Kind + name of one top-level statement, mirroring R assignment spellings:
/// `<-`, `=`, `<<-`, `:=` bind on the left, `->`, `->>` on the right.
fn classify_top_level(node: &syntax::SyntaxNode) -> (ItemKind, Option<String>) {
    use syntax::SyntaxKind;
    if node.kind() != SyntaxKind::BINARY_EXPR {
        return (ItemKind::Statement, None);
    }
    let binary = syntax::ast::BinaryExpr::cast(node.clone());
    let Some(binary) = binary else { return (ItemKind::Statement, None) };
    let Some(operator) = binary.operator() else { return (ItemKind::Statement, None) };
    let (target, value) = match operator.kind() {
        SyntaxKind::LESS_MINUS | SyntaxKind::EQ | SyntaxKind::LESS2_MINUS | SyntaxKind::COLON_EQ => {
            (binary.lhs(), binary.rhs())
        }
        SyntaxKind::MINUS_GREATER | SyntaxKind::MINUS_GREATER2 => (binary.rhs(), binary.lhs()),
        _ => return (ItemKind::Statement, None),
    };
    let name = target
        .clone()
        .and_then(syntax::ast::Name::cast)
        .and_then(|name| name.text())
        .or_else(|| {
            // A string target (`"name" <- ...`) defines the unquoted name.
            target.as_ref().and_then(|node| {
                (node.kind() == SyntaxKind::LITERAL
                    && node.first_token().is_some_and(|t| t.kind() == SyntaxKind::STRING))
                .then(|| {
                    let text = node.text().to_string();
                    text.trim_matches(['"', '\'']).to_owned()
                })
            })
        });
    if name.is_none() {
        return (ItemKind::Statement, None);
    }
    let kind = match value.map(|value| value.kind()) {
        Some(SyntaxKind::FUNCTION_DEF) => ItemKind::Function,
        _ => ItemKind::Value,
    };
    (kind, name)
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

    #[test]
    fn item_identity_and_position_independence() {
        let mut db = RootDatabase::default();
        let file = SourceFile::new(
            &db,
            "f <- function(x) x\ng <- function(y) y\n".to_owned(),
            DocumentKind::Package,
        );
        let g_before = {
            let items = item_tree(&db, file);
            assert_eq!(items.len(), 2);
            assert_eq!(items[1].name(&db).as_deref(), Some("g"));
            item_syntax(&db, items[1]).expect("g exists")
        };

        // An edit inside `f`'s body shifts `g` — its item identity (the
        // interned id re-minted from the same fields) and its green subtree
        // must both survive unchanged: structural equality across shifted
        // offsets is what early cutoff rests on.
        file.set_text(&mut db).to("f <- function(x) x + 100\ng <- function(y) y\n".to_owned());
        {
            let items = item_tree(&db, file);
            assert_eq!(items.len(), 2);
            let g = Item::new(&db, file, ItemKind::Function, Some("g".to_owned()), None, 0);
            assert_eq!(items[1], g);
            let g_after = item_syntax(&db, g).expect("g still exists");
            assert_eq!(g_before, g_after);
        }

        // Deleting `g` resolves its item to nothing.
        file.set_text(&mut db).to("f <- function(x) x\n".to_owned());
        let g = Item::new(&db, file, ItemKind::Function, Some("g".to_owned()), None, 0);
        assert_eq!(item_syntax(&db, g), None);
    }

    #[test]
    fn hir_lowers_and_survives_shifts() {
        let mut db = RootDatabase::default();
        let file = SourceFile::new(
            &db,
            "pad <- 1\nadd <- function(x, y = 2) x + y\n".to_owned(),
            DocumentKind::Package,
        );
        let add = Item::new(&db, file, ItemKind::Function, Some("add".to_owned()), None, 0);
        let before = item_hir(&db, add).expect("add lowers");
        let root = before.expression(before.root.expect("has root"));
        let hir::ExpressionKind::Assign { value, .. } = &root.kind else {
            panic!("expected an assignment root, got {root:?}");
        };
        let hir::ExpressionKind::Function { parameters, body } =
            &before.expression(*value).kind
        else {
            panic!("expected a function value");
        };
        assert_eq!(parameters.len(), 2);
        assert_eq!(parameters[0].name, "x");
        assert_eq!(parameters[1].name, "y");
        assert!(parameters[1].default.is_some());
        let hir::ExpressionKind::Binary { operator, .. } = &before.expression(*body).kind else {
            panic!("expected a binary body");
        };
        assert_eq!(*operator, hir::BinaryOperator::Add);

        // Shifting `add` (an edit in the item before it) reproduces an equal
        // Module: spans are item-relative, so nothing downstream re-runs.
        file.set_text(&mut db)
            .to("pad <- 100000\nadd <- function(x, y = 2) x + y\n".to_owned());
        let add = Item::new(&db, file, ItemKind::Function, Some("add".to_owned()), None, 0);
        let after = item_hir(&db, add).expect("add still lowers");
        assert_eq!(before, after);
    }
}
