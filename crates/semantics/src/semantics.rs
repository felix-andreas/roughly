//! Semantic analysis over `syntax` trees: the salsa database and all queries.
//!
//! The pipeline is a set of memoized, dependency-tracked queries: parse →
//! per-item item tree → HIR → naming → inference, with interned types and a
//! symbol-granular package interface resolved through salsa fixpoint cycles.
//! Analysis units are *items* (top-level definitions and nested definitions in
//! class-constructor calls and function bodies), never whole files, so an edit
//! recomputes only the items whose derived values actually changed.

pub mod annotations;
pub mod check;
pub mod diagnostics;
pub mod hir;
pub mod infer;
pub mod naming;
pub mod stubs;
pub mod types;

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
    resolve_item_node(db, item).map(|node| ItemSyntax(node.green().into()))
}

/// The item's current red node inside the FILE tree (absolute offsets) — the
/// rendering edge uses this to re-anchor item-relative spans; everything else
/// must go through the position-independent `item_syntax`.
pub(crate) fn resolve_item_node<'db>(
    db: &'db dyn Db,
    item: Item<'db>,
) -> Option<syntax::SyntaxNode> {
    let parse = parse(db, *item.file(db));
    let root = parse.syntax_node();
    let mut counts: rustc_hash::FxHashMap<(ItemKind, Option<String>), u32> =
        rustc_hash::FxHashMap::default();
    let target = (
        *item.kind(db),
        item.name(db).clone(),
        *item.disambiguator(db),
    );
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
            return Some(node);
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

/// The naming facts of one item (position-independent, like the HIR they are
/// derived from).
#[salsa::tracked(returns(clone))]
pub fn item_naming<'db>(db: &'db dyn Db, item: Item<'db>) -> Option<naming::ItemNaming> {
    let module = item_hir(db, item)?;
    Some(naming::resolve_item(&module))
}

/// Whether any global definition with this name exists — a package definition
/// or a stdlib stub declaration (used to silence could-not-resolve on names
/// the interface will serve).
pub fn package_scheme_exists(db: &dyn Db, name: &str) -> bool {
    if ProjectFiles::try_get(db)
        .map(|files| package_definitions(db, files).contains_key(name))
        .unwrap_or(false)
    {
        return true;
    }
    stubs::stubs(db).is_some_and(|library| {
        library.schemes.contains_key(name) || library.nominals.contains(name)
    })
}

/// The annotation region immediately preceding an item (only trivia between),
/// as a position-independent green subtree. Annotations are siblings of the
/// item statement in the file tree, so attachment happens here, not inside
/// `item_syntax`.
#[salsa::tracked(returns(clone))]
pub fn item_annotation_syntax<'db>(db: &'db dyn Db, item: Item<'db>) -> Option<ItemSyntax> {
    let parse = parse(db, *item.file(db));
    let root = parse.syntax_node();
    let mut counts: rustc_hash::FxHashMap<(ItemKind, Option<String>), u32> =
        rustc_hash::FxHashMap::default();
    let target = (
        *item.kind(db),
        item.name(db).clone(),
        *item.disambiguator(db),
    );
    let mut pending_annotation: Option<syntax::SyntaxNode> = None;
    for child in root.children_with_tokens() {
        match child {
            rowan::NodeOrToken::Node(node) if node.kind() == syntax::SyntaxKind::ANNOTATION => {
                pending_annotation = Some(node);
            }
            rowan::NodeOrToken::Node(node)
                if syntax::ast::is_expression_kind(node.kind())
                    || node.kind() == syntax::SyntaxKind::ERROR =>
            {
                let (kind, name) = classify_top_level(&node);
                let counter = counts.entry((kind, name.clone())).or_insert(0);
                let disambiguator = *counter;
                *counter += 1;
                if (kind, name, disambiguator) == target {
                    return pending_annotation.map(|node| ItemSyntax(node.green().into()));
                }
                pending_annotation = None;
            }
            rowan::NodeOrToken::Token(token)
                if matches!(
                    token.kind(),
                    syntax::SyntaxKind::WHITESPACE | syntax::SyntaxKind::NEWLINE
                ) => {}
            _ => pending_annotation = None,
        }
    }
    None
}

/// The ordered project file set (package files first, in path order — the
/// order that decides last-writer-wins winners). A singleton input the host
/// keeps current.
#[salsa::input(singleton, debug)]
pub struct ProjectFiles {
    #[returns(ref)]
    pub files: Vec<SourceFile>,
}

/// The winning definition item per package-exported name: later files (and
/// later assignments within one file) override earlier ones.
#[salsa::tracked(returns(clone))]
pub fn package_definitions<'db>(
    db: &'db dyn Db,
    files: ProjectFiles,
) -> rustc_hash::FxHashMap<String, Item<'db>> {
    let mut winners = rustc_hash::FxHashMap::default();
    for &file in files.files(db) {
        if *file.kind(db) != DocumentKind::Package {
            continue;
        }
        for item in item_tree(db, file) {
            if matches!(*item.kind(db), ItemKind::Function | ItemKind::Value)
                && let Some(name) = item.name(db).clone()
            {
                winners.insert(name, item);
            }
        }
    }
    winners
}

/// The exported scheme of one definition item. Cross-item references route
/// through this query, so mutually recursive definitions form salsa cycles:
/// fixpoint iteration starts every member at the tolerant `Unknown` scheme and
/// iterates to convergence; a non-converging (oscillating) member pins to
/// `Unknown` at the round cap instead of ever reaching salsa's hard limit.
#[salsa::tracked(
    returns(clone),
    cycle_fn = global_scheme_recover,
    cycle_initial = global_scheme_initial
)]
pub fn global_scheme<'db>(db: &'db dyn Db, item: Item<'db>) -> types::TypeScheme<'db> {
    item_check(db, item)
        .and_then(|check| check.scheme)
        .unwrap_or_else(|| types::TypeScheme::monomorphic(types::unknown(db)))
}

fn global_scheme_initial<'db>(
    db: &'db dyn Db,
    _id: salsa::Id,
    _item: Item<'db>,
) -> types::TypeScheme<'db> {
    types::TypeScheme::monomorphic(types::unknown(db))
}

/// Bound fixpoint rounds mirroring the legacy interface round cap: a scheme
/// still changing after the cap pins to `Unknown` (sound-by-refusal) rather
/// than iterating toward salsa's panic limit.
const SCHEME_ROUND_CAP: u32 = 16;

fn global_scheme_recover<'db>(
    db: &'db dyn Db,
    cycle: &salsa::Cycle,
    last_provisional: &types::TypeScheme<'db>,
    value: types::TypeScheme<'db>,
    _item: Item<'db>,
) -> types::TypeScheme<'db> {
    if &value == last_provisional {
        return value;
    }
    if cycle.iteration() >= SCHEME_ROUND_CAP {
        return types::TypeScheme::monomorphic(types::unknown(db));
    }
    value
}

/// The full per-item check: HIR + naming + annotation lowering + inference,
/// with cross-item reads resolved through `global_scheme` (the cycle-aware
/// interface edge). Derived from position-independent per-item values only,
/// so it cuts off whenever the item (and its annotation) are untouched.
#[salsa::tracked(
    returns(clone),
    cycle_fn = item_check_recover,
    cycle_initial = item_check_initial
)]
pub fn item_check<'db>(db: &'db dyn Db, item: Item<'db>) -> Option<check::ItemCheck<'db>> {
    let module = item_hir(db, item)?;
    let naming = item_naming(db, item)?;
    let annotation = item_annotation_syntax(db, item)
        .map(|syntax| annotations::lower_annotation(db, &syntax.syntax_node()));
    let globals = SalsaGlobals {
        db,
        definitions: ProjectFiles::try_get(db).map(|files| package_definitions(db, files)),
    };
    Some(check::check_item_with_annotation(
        db,
        &module,
        &naming,
        annotation.as_ref(),
        Some(&globals),
    ))
}

fn item_check_initial<'db>(
    _db: &'db dyn Db,
    _id: salsa::Id,
    _item: Item<'db>,
) -> Option<check::ItemCheck<'db>> {
    None
}

fn item_check_recover<'db>(
    _db: &'db dyn Db,
    _cycle: &salsa::Cycle,
    _last_provisional: &Option<check::ItemCheck<'db>>,
    value: Option<check::ItemCheck<'db>>,
    _item: Item<'db>,
) -> Option<check::ItemCheck<'db>> {
    // Convergence is decided at the `global_scheme` edge; the check value
    // simply follows the iteration.
    value
}

/// The salsa-backed cross-item resolver handed to the checker.
struct SalsaGlobals<'db> {
    db: &'db dyn Db,
    definitions: Option<rustc_hash::FxHashMap<String, Item<'db>>>,
}

impl<'db> check::GlobalEnv<'db> for SalsaGlobals<'db> {
    fn scheme(&self, name: &str) -> Option<types::TypeScheme<'db>> {
        if let Some(item) = self
            .definitions
            .as_ref()
            .and_then(|winners| winners.get(name))
        {
            return Some(global_scheme(self.db, *item));
        }
        // Until overload-set probing lands, a stub name resolves to its last
        // candidate: the corpus orders candidates specific-first, so the last
        // one is the most general fallback.
        let library = stubs::stubs(self.db)?;
        library.schemes.get(name)?.last().cloned()
    }
}

/// Kind + name of one top-level statement, mirroring R assignment spellings:
/// `<-`, `=`, `<<-`, `:=` bind on the left, `->`, `->>` on the right.
fn classify_top_level(node: &syntax::SyntaxNode) -> (ItemKind, Option<String>) {
    use syntax::SyntaxKind;
    if node.kind() != SyntaxKind::BINARY_EXPR {
        return (ItemKind::Statement, None);
    }
    let binary = syntax::ast::BinaryExpr::cast(node.clone());
    let Some(binary) = binary else {
        return (ItemKind::Statement, None);
    };
    let Some(operator) = binary.operator() else {
        return (ItemKind::Statement, None);
    };
    let (target, value) = match operator.kind() {
        SyntaxKind::LESS_MINUS
        | SyntaxKind::EQ
        | SyntaxKind::LESS2_MINUS
        | SyntaxKind::COLON_EQ => (binary.lhs(), binary.rhs()),
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
                    && node
                        .first_token()
                        .is_some_and(|t| t.kind() == SyntaxKind::STRING))
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
        file.set_text(&mut db)
            .to("f <- function(x) x + 100\ng <- function(y) y\n".to_owned());
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
        let add = Item::new(
            &db,
            file,
            ItemKind::Function,
            Some("add".to_owned()),
            None,
            0,
        );
        let before = item_hir(&db, add).expect("add lowers");
        let root = before.expression(before.root.expect("has root"));
        let hir::ExpressionKind::Assign { value, .. } = &root.kind else {
            panic!("expected an assignment root, got {root:?}");
        };
        let hir::ExpressionKind::Function { parameters, body } = &before.expression(*value).kind
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
        let add = Item::new(
            &db,
            file,
            ItemKind::Function,
            Some("add".to_owned()),
            None,
            0,
        );
        let after = item_hir(&db, add).expect("add still lowers");
        assert_eq!(before, after);
    }

    #[test]
    fn cross_file_reads_resolve_through_the_interface() {
        let mut db = RootDatabase::default();
        let util = SourceFile::new(
            &db,
            "add <- function(x, y) x + y\n".to_owned(),
            DocumentKind::Package,
        );
        let main = SourceFile::new(
            &db,
            "use <- function() add(1L, 2L)\nbad <- function() add(\"a\", 2L)\n".to_owned(),
            DocumentKind::Package,
        );
        ProjectFiles::new(&db, vec![util, main]);

        let use_item = Item::new(
            &db,
            main,
            ItemKind::Function,
            Some("use".to_owned()),
            None,
            0,
        );
        let use_check = item_check(&db, use_item).expect("use checks");
        assert!(use_check.errors.is_empty(), "{:?}", use_check.errors);

        let bad_item = Item::new(
            &db,
            main,
            ItemKind::Function,
            Some("bad".to_owned()),
            None,
            0,
        );
        let bad_check = item_check(&db, bad_item).expect("bad checks");
        assert!(
            !bad_check.errors.is_empty(),
            "character into numeric parameter must report"
        );

        // Editing the callee body (same shape) leaves the caller check equal.
        util.set_text(&mut db)
            .to("add <- function(x, y) y + x\n".to_owned());
        let use_item = Item::new(
            &db,
            main,
            ItemKind::Function,
            Some("use".to_owned()),
            None,
            0,
        );
        let again = item_check(&db, use_item).expect("use still checks");
        assert!(again.errors.is_empty());
    }

    #[test]
    fn mutual_recursion_converges_through_the_fixpoint() {
        let db = RootDatabase::default();
        let file = SourceFile::new(
            &db,
            "is_even <- function(n) if (n == 0L) TRUE else is_odd(n - 1L)\nis_odd <- function(n) if (n == 0L) FALSE else is_even(n - 1L)\n".to_owned(),
            DocumentKind::Package,
        );
        ProjectFiles::new(&db, vec![file]);
        let even = Item::new(
            &db,
            file,
            ItemKind::Function,
            Some("is_even".to_owned()),
            None,
            0,
        );
        let check = item_check(&db, even).expect("checks");
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        let scheme = global_scheme(&db, even);
        assert!(
            matches!(scheme.body.kind(&db), types::TyKind::Function(_)),
            "mutual recursion must converge to a function scheme, got {:?}",
            scheme.body.kind(&db)
        );
    }

    #[test]
    fn self_recursion_stays_tolerant() {
        let db = RootDatabase::default();
        let file = SourceFile::new(
            &db,
            "count <- function(n) if (n == 0L) 0L else count(n - 1L)\n".to_owned(),
            DocumentKind::Package,
        );
        ProjectFiles::new(&db, vec![file]);
        let item = Item::new(
            &db,
            file,
            ItemKind::Function,
            Some("count".to_owned()),
            None,
            0,
        );
        // Must not panic; the self-cycle iterates to a stable scheme.
        let check = item_check(&db, item).expect("checks");
        assert!(check.errors.is_empty(), "{:?}", check.errors);
    }

    #[test]
    fn later_file_wins_the_definition() {
        let db = RootDatabase::default();
        let first = SourceFile::new(&db, "limit <- \"low\"\n".to_owned(), DocumentKind::Package);
        let second = SourceFile::new(&db, "limit <- 10L\n".to_owned(), DocumentKind::Package);
        let files = ProjectFiles::new(&db, vec![first, second]);
        let winners = package_definitions(&db, files);
        let winner = winners.get("limit").expect("winner exists");
        assert_eq!(*winner.file(&db), second);
        let scheme = global_scheme(&db, *winner);
        assert_eq!(
            scheme.body,
            types::scalar(&db, types::Atomic::Integer),
            "the later file's integer wins"
        );
    }
}
