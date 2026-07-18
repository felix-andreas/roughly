//! IDE features over the semantics crate's salsa queries. Every feature is a
//! pure read of the database at a byte offset; UTF-16 and editor-protocol
//! concerns live in the server, not here.
//!
//! Positions cross one boundary: per-item query results carry item-relative
//! ranges (the position-independent unit salsa cutoffs work on), while the
//! feature API speaks file-absolute offsets. `semantics::item_node` is the
//! edge that anchors an item at its current absolute position.

use semantics::diagnostics::TypeRenderer;
use semantics::hir::{ExprId, ExpressionKind};
use semantics::naming::BindingId;
use semantics::{
    Db, Item, ItemKind, ProjectFiles, SourceFile, item_check, item_hir, item_naming, item_node,
    item_tree, package_definitions,
};
use syntax::{TextRange, TextSize};

/// A hover result: the hovered expression's absolute range and the rendered
/// lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    pub range: TextRange,
    pub lines: Vec<String>,
}

/// A navigation target: a file and an absolute range inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavigationTarget {
    pub file: SourceFile,
    pub range: TextRange,
}

pub fn hover(db: &dyn Db, file: SourceFile, offset: TextSize) -> Option<Hover> {
    let position = position_in_item(db, file, offset)?;
    let expression = position.expression_at()?;
    let check = item_check(db, position.item)?;
    let ty = *check.expression_types.get(&expression)?;

    let hir = item_hir(db, position.item)?;
    let mut renderer = TypeRenderer::default();
    let line = match &hir.expression(expression).kind {
        ExpressionKind::NameRef(name) => {
            format!("{name}: {}", renderer.render(db, ty))
        }
        _ => renderer.render(db, ty),
    };

    let range = hir.expression(expression).range + position.item_offset;
    Some(Hover {
        range,
        lines: vec![line],
    })
}

pub fn definition(
    db: &dyn Db,
    files: ProjectFiles,
    file: SourceFile,
    offset: TextSize,
) -> Option<NavigationTarget> {
    let position = position_in_item(db, file, offset)?;
    let expression = position.expression_at()?;
    let naming = item_naming(db, position.item)?;

    if let Some(binding) = naming.resolutions.get(&expression) {
        let info = naming.bindings.get(binding)?;
        return Some(NavigationTarget {
            file,
            range: info.range + position.item_offset,
        });
    }

    // A read no lexical slot resolves: a package global defined in another
    // item or file.
    let name = naming.non_locals.get(&expression)?;
    let target = *package_definitions(db, files).get(name)?;
    let node = item_node(db, target)?;
    let range = definition_name_range(&node).unwrap_or_else(|| node.text_range());
    Some(NavigationTarget {
        file: *target.file(db),
        range,
    })
}

/// All occurrences of the slot under the cursor (reads and assignment
/// targets) inside its item, in source order. Cross-item and cross-file
/// global references are a later slice.
pub fn references(db: &dyn Db, file: SourceFile, offset: TextSize) -> Vec<NavigationTarget> {
    let Some(position) = position_in_item(db, file, offset) else {
        return Vec::new();
    };
    let Some(expression) = position.expression_at() else {
        return Vec::new();
    };
    let Some(naming) = item_naming(db, position.item) else {
        return Vec::new();
    };
    let Some(binding) = naming.resolutions.get(&expression).copied() else {
        return Vec::new();
    };
    slot_occurrences(db, position.item, position.item_offset, binding)
        .into_iter()
        .map(|range| NavigationTarget { file, range })
        .collect()
}

/// Document symbols: the file's named top-level definitions with their
/// absolute name ranges, in source order.
pub fn document_symbols(db: &dyn Db, file: SourceFile) -> Vec<(String, NavigationTarget)> {
    let mut symbols = Vec::new();
    for item in item_tree(db, file) {
        if !matches!(*item.kind(db), ItemKind::Function | ItemKind::Value) {
            continue;
        }
        let Some(name) = item.name(db).clone() else {
            continue;
        };
        let Some(node) = item_node(db, item) else {
            continue;
        };
        let range = definition_name_range(&node).unwrap_or_else(|| node.text_range());
        symbols.push((name, NavigationTarget { file, range }));
    }
    symbols
}

/// Every occurrence range of one binding slot inside an item, absolute.
fn slot_occurrences(
    db: &dyn Db,
    item: Item<'_>,
    item_offset: TextSize,
    binding: BindingId,
) -> Vec<TextRange> {
    let Some(naming) = item_naming(db, item) else {
        return Vec::new();
    };
    let Some(hir) = item_hir(db, item) else {
        return Vec::new();
    };
    let mut ranges: Vec<TextRange> = naming
        .resolutions
        .iter()
        .filter(|(_, resolved)| **resolved == binding)
        .map(|(expression, _)| hir.expression(*expression).range + item_offset)
        .collect();
    ranges.sort_by_key(|range| range.start());
    ranges.dedup();
    ranges
}

/// The cursor's item and the item-relative cursor offset.
struct PositionedItem<'db> {
    db: &'db dyn Db,
    item: Item<'db>,
    item_offset: TextSize,
    relative: TextSize,
}

impl PositionedItem<'_> {
    /// The smallest HIR expression whose range contains the cursor —
    /// end-inclusive, so a cursor sitting immediately after a name still hits
    /// it (the editor convention). Name references win ties.
    fn expression_at(&self) -> Option<ExprId> {
        let hir = item_hir(self.db, self.item)?;
        let mut best: Option<(TextSize, bool, ExprId)> = None;
        for (index, expression) in hir.expressions.iter().enumerate() {
            let range = expression.range;
            if range.is_empty() || !(range.start() <= self.relative && self.relative <= range.end())
            {
                continue;
            }
            let key = (
                range.len(),
                !matches!(expression.kind, ExpressionKind::NameRef(_)),
            );
            if best
                .as_ref()
                .is_none_or(|(width, not_name, _)| key < (*width, *not_name))
            {
                best = Some((key.0, key.1, ExprId(index as u32)));
            }
        }
        best.map(|(_, _, expression)| expression)
    }
}

fn position_in_item(db: &dyn Db, file: SourceFile, offset: TextSize) -> Option<PositionedItem<'_>> {
    for item in item_tree(db, file) {
        let Some(node) = item_node(db, item) else {
            continue;
        };
        let range = node.text_range();
        if range.start() <= offset && offset < range.end() {
            return Some(PositionedItem {
                db,
                item,
                item_offset: range.start(),
                relative: offset - range.start(),
            });
        }
    }
    None
}

/// The name node range of a definition item (`name <- ...`), for goto
/// targets that land on the name rather than the whole statement.
fn definition_name_range(node: &syntax::SyntaxNode) -> Option<TextRange> {
    node.descendants()
        .find(|descendant| descendant.kind() == syntax::SyntaxKind::NAME)
        .map(|name| name.text_range())
}
