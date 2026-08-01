//! The analysis core behind the zoomable code canvas.
//!
//! It turns a set of R sources into one flat, renderable index: every
//! top-level definition with its source text, its classified tokens, its
//! resolved references, and the reference edges between definitions. The
//! canvas front end never parses R — it only lays out and draws what this
//! index describes, so what you see on screen is exactly what ry's parser,
//! name resolution, and type checker concluded.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ide::DocumentSymbolKind;
use semantics::{
    DocumentKind, Item, ItemKind, ProjectFiles, RootDatabase, SourceFile,
    diagnostics::{Severity, file_diagnostics},
    item_hir, item_naming, item_node, item_tree,
    metadata::{
        PackageMetadata, attached_union, normalized_imports, parse_description_dependencies,
        parse_description_package, parse_namespace_imports,
    },
    package_definitions,
    stubs::{StubSources, shipped_export_manifests, shipped_stub_sources},
};
use serde::{Deserialize, Serialize};
use syntax::{SyntaxKind, SyntaxNode, TextRange, TextSize};

/// Everything the canvas draws, in one payload.
#[derive(Debug, Default, Serialize)]
pub struct Index {
    pub files: Vec<FileNode>,
    pub items: Vec<ItemNode>,
    /// Reference edges between definitions, deduplicated. `[from, to]` pairs.
    pub edges: Vec<[u32; 2]>,
}

#[derive(Debug, Serialize)]
pub struct FileNode {
    pub path: String,
    pub lines: u32,
    /// Indices into [`Index::items`], in source order.
    pub items: Vec<u32>,
}

/// One top-level definition: the unit the canvas draws as a card.
#[derive(Debug, Serialize)]
pub struct ItemNode {
    pub file: u32,
    pub name: String,
    pub kind: &'static str,
    /// `function(x, y)` for functions, the signature classes for S4 methods,
    /// the declared spelling for `#:` type declarations.
    pub signature: Option<String>,
    /// The item's inferred type, rendered the way hover renders it.
    pub type_rendering: Option<String>,
    /// The first sentence of the leading roxygen block.
    pub doc: Option<String>,
    /// The definition's own source text. Every offset below indexes into it.
    pub code: String,
    /// The item's first line in its file, 0-based, for editor hand-off.
    pub line: u32,
    /// Classified tokens as flat `[start, length, class]` triples — flat
    /// because a struct per token triples the payload on real packages.
    pub tokens: Vec<u32>,
    /// Resolved references to other definitions, as flat
    /// `[start, length, target_item]` triples. These are the sites the
    /// canvas expands inline.
    pub references: Vec<u32>,
    pub errors: u32,
    pub warnings: u32,
}

/// Token classes the front end colors. Kept as a closed set of small
/// integers: the palette lives in the renderer, not here.
pub const TOKEN_PLAIN: u32 = 0;
pub const TOKEN_KEYWORD: u32 = 1;
pub const TOKEN_STRING: u32 = 2;
pub const TOKEN_NUMBER: u32 = 3;
pub const TOKEN_COMMENT: u32 = 4;
pub const TOKEN_OPERATOR: u32 = 5;
pub const TOKEN_PUNCTUATION: u32 = 6;
pub const TOKEN_CALLEE: u32 = 7;
pub const TOKEN_ANNOTATION: u32 = 8;
pub const TOKEN_NAMESPACE: u32 = 9;

/// One analysis request: the project's sources plus, for a package, the two
/// metadata files that decide which namespaces resolve.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub files: Vec<RequestFile>,
    pub description: Option<String>,
    pub namespace: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RequestFile {
    /// The project-relative path. It names the file frame on the canvas and
    /// decides last-writer-wins order among competing definitions.
    pub path: String,
    pub text: String,
}

/// Externally tagged so a response is either an index or a failure, never a
/// half of each.
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Response {
    Index(Index),
    Error(String),
}

/// Reserve `length` bytes for the host to write a request into.
///
/// # Safety
/// The returned pointer is valid for `length` bytes until it is handed to
/// [`canvas_analyze`], which takes ownership of it.
#[unsafe(no_mangle)]
pub extern "C" fn canvas_alloc(length: usize) -> *mut u8 {
    let mut buffer = Vec::<u8>::with_capacity(length);
    let pointer = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    pointer
}

/// Analyze the UTF-8 JSON [`Request`] at `request`, and return a buffer whose
/// first four bytes hold the response length little-endian, with the JSON
/// [`Response`] following. The host reads it and returns it to
/// [`canvas_release`].
///
/// A raw C ABI rather than a generated binding layer: the whole surface is one
/// string in, one string out, so wasm-bindgen would add a build step and a
/// toolchain to install in exchange for nothing.
///
/// # Safety
/// `request` must come from [`canvas_alloc`] with the same `length`, and must
/// not be used again afterwards — this call takes ownership of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn canvas_analyze(request: *mut u8, length: usize) -> *mut u8 {
    let request = unsafe { Vec::from_raw_parts(request, length, length) };
    let response = match std::str::from_utf8(&request)
        .map_err(|error| error.to_string())
        .and_then(|text| serde_json::from_str::<Request>(text).map_err(|error| error.to_string()))
    {
        Ok(request) => {
            let sources: Vec<(String, String)> = request
                .files
                .into_iter()
                .map(|file| (file.path, file.text))
                .collect();
            Response::Index(build(&sources, &request.description, &request.namespace))
        }
        Err(error) => Response::Error(error),
    };

    // A serialization failure here cannot be reported through the same
    // channel, so it degrades to the smallest valid response the host can
    // still parse.
    let payload = serde_json::to_vec(&response)
        .unwrap_or_else(|_| br#"{"error":"the index could not be serialized"}"#.to_vec());
    let mut buffer = Vec::with_capacity(4 + payload.len());
    buffer.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buffer.extend_from_slice(&payload);
    let mut buffer = buffer.into_boxed_slice();
    let pointer = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    pointer
}

/// Release a buffer returned by [`canvas_analyze`].
///
/// # Safety
/// `response` must be a pointer [`canvas_analyze`] returned and not yet
/// released, and `length` must be the payload length it recorded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn canvas_release(response: *mut u8, length: usize) {
    drop(unsafe { Vec::from_raw_parts(response, 4 + length, 4 + length) });
}

/// Analyze `sources` and project the result into the canvas index.
pub fn build(
    sources: &[(String, String)],
    description: &Option<String>,
    namespace: &Option<String>,
) -> Index {
    let mut database = RootDatabase::default();
    StubSources::new(
        &database,
        shipped_stub_sources(),
        shipped_export_manifests(),
    );

    let namespace_imports = namespace
        .as_deref()
        .map(parse_namespace_imports)
        .unwrap_or_default();
    let metadata = PackageMetadata::new(
        &database,
        normalized_imports(&namespace_imports),
        description
            .as_deref()
            .map(parse_description_dependencies)
            .unwrap_or_default(),
        BTreeSet::new(),
        description.as_deref().and_then(parse_description_package),
    );

    // Path order decides last-writer-wins among competing definitions, so the
    // canvas must index in the same order the CLI checks in.
    let mut ordered: Vec<usize> = (0..sources.len()).collect();
    ordered.sort_by(|&left, &right| sources[left].0.cmp(&sources[right].0));

    let project: Vec<SourceFile> = ordered
        .iter()
        .map(|&source| SourceFile::new(&database, sources[source].1.clone(), DocumentKind::Package))
        .collect();

    // Attachment is read off the parses alone, so it can be settled before the
    // project file set exists — which it must be, since the metadata input is
    // a singleton and every later query would otherwise see it half-built.
    let attached = attached_union(&database, project.clone());
    if !attached.is_empty() {
        use salsa::Setter as _;
        metadata.set_attached(&mut database).to(attached);
    }

    let database = &database;
    let files = ProjectFiles::new(database, project.clone());

    // Pass one assigns every definition its canvas id, because a reference
    // resolved in one file routinely targets a definition in a later one.
    let mut item_ids: HashMap<Item<'_>, u32> = HashMap::new();
    let mut ordered_items: Vec<(u32, Item<'_>)> = Vec::new();
    for (file_index, &file) in project.iter().enumerate() {
        for &item in item_tree(database, file) {
            if item.name(database).is_none() {
                continue;
            }
            item_ids.insert(item, ordered_items.len() as u32);
            ordered_items.push((file_index as u32, item));
        }
    }

    let winners = package_definitions(database, files);
    let mut index = Index {
        files: Vec::with_capacity(project.len()),
        items: Vec::with_capacity(ordered_items.len()),
        edges: Vec::new(),
    };

    let mut items_by_file: Vec<Vec<u32>> = vec![Vec::new(); project.len()];
    let mut edges: BTreeSet<[u32; 2]> = BTreeSet::new();

    for (identifier, &(file_index, item)) in ordered_items.iter().enumerate() {
        let identifier = identifier as u32;
        let Some(node) = item_node(database, item) else {
            continue;
        };
        let file = project[file_index as usize];
        let text = file.text(database);
        let span = node.text_range();
        let offset = span.start();
        let code = text[usize::from(span.start())..usize::from(span.end())].to_owned();

        let mut references = Vec::new();
        if let (Some(hir), Some(naming)) = (item_hir(database, item), item_naming(database, item)) {
            let mut sites: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
            for (&expression, name) in &naming.non_locals {
                let Some(&winner) = winners.get(name) else {
                    continue;
                };
                let Some(&target) = item_ids.get(&winner) else {
                    continue;
                };
                let range = hir.expression(expression).range;
                sites.insert(u32::from(range.start()), (range.len().into(), target));
                if target != identifier {
                    edges.insert([identifier, target]);
                }
            }
            for (start, (length, target)) in sites {
                references.extend_from_slice(&[start, length, target]);
            }
        }

        let symbol = ide::document_symbols(database, file)
            .into_iter()
            .find(|symbol| symbol.range.start() == span.start());
        let kind = symbol
            .as_ref()
            .map(|symbol| symbol_kind(symbol.kind))
            .unwrap_or_else(|| item_kind(*item.kind(database)));

        let (errors, warnings) = diagnostic_counts(database, file, span);
        index.items.push(ItemNode {
            file: file_index,
            name: item.name(database).clone().unwrap_or_default(),
            kind,
            signature: symbol.as_ref().and_then(|symbol| symbol.detail.clone()),
            type_rendering: type_rendering(database, files, file, &node),
            doc: leading_doc(text, span.start()),
            line: line_of(text, span.start()),
            tokens: tokens(&node, offset),
            code,
            references,
            errors,
            warnings,
        });
        items_by_file[file_index as usize].push(identifier);
    }

    for (file_index, &file) in project.iter().enumerate() {
        let source = ordered[file_index];
        index.files.push(FileNode {
            path: sources[source].0.clone(),
            lines: file.text(database).lines().count() as u32,
            items: std::mem::take(&mut items_by_file[file_index]),
        });
    }
    index.edges = edges.into_iter().collect();
    index
}

/// Classify every token under `node` into the renderer's palette slots.
/// Offsets come back item-relative, so a card can highlight its own text
/// without knowing where in the file it sits.
fn tokens(node: &SyntaxNode, offset: TextSize) -> Vec<u32> {
    let callees = callee_ranges(node);
    let mut tokens = Vec::new();
    for element in node.descendants_with_tokens() {
        let Some(token) = element.into_token() else {
            continue;
        };
        let kind = token.kind();
        if kind == SyntaxKind::WHITESPACE || kind == SyntaxKind::NEWLINE {
            continue;
        }
        let range = token.text_range();
        let class = if token
            .parent_ancestors()
            .any(|ancestor| ancestor.kind() == SyntaxKind::ANNOTATION)
            || kind == SyntaxKind::ANNOTATION_MARKER
        {
            TOKEN_ANNOTATION
        } else if kind == SyntaxKind::COMMENT {
            TOKEN_COMMENT
        } else if kind.is_keyword() {
            TOKEN_KEYWORD
        } else if matches!(kind, SyntaxKind::STRING | SyntaxKind::RAW_STRING) {
            TOKEN_STRING
        } else if matches!(
            kind,
            SyntaxKind::INTEGER | SyntaxKind::DOUBLE | SyntaxKind::COMPLEX
        ) {
            TOKEN_NUMBER
        } else if callees.contains(&u32::from(range.start())) {
            TOKEN_CALLEE
        } else if is_namespace_qualifier(&token) {
            TOKEN_NAMESPACE
        } else if matches!(
            kind,
            SyntaxKind::IDENT | SyntaxKind::DOTS | SyntaxKind::DOTDOTI | SyntaxKind::UNDERSCORE
        ) {
            TOKEN_PLAIN
        } else if matches!(
            kind,
            SyntaxKind::L_PAREN
                | SyntaxKind::R_PAREN
                | SyntaxKind::L_BRACE
                | SyntaxKind::R_BRACE
                | SyntaxKind::L_BRACKET
                | SyntaxKind::L_BRACKET2
                | SyntaxKind::R_BRACKET
                | SyntaxKind::COMMA
                | SyntaxKind::SEMICOLON
        ) {
            TOKEN_PUNCTUATION
        } else {
            TOKEN_OPERATOR
        };
        tokens.extend_from_slice(&[u32::from(range.start() - offset), range.len().into(), class]);
    }
    tokens
}

/// The ranges of names in callee position, including the trailing name of a
/// `pkg::fun(…)` qualifier. Collected up front because deciding it per token
/// would re-walk the call's children for every identifier in the item.
fn callee_ranges(node: &SyntaxNode) -> BTreeSet<u32> {
    let mut ranges = BTreeSet::new();
    for descendant in node.descendants() {
        if descendant.kind() != SyntaxKind::CALL_EXPR {
            continue;
        }
        let Some(callee) = descendant.first_child() else {
            continue;
        };
        match callee.kind() {
            SyntaxKind::NAME => {
                ranges.insert(callee.text_range().start().into());
            }
            SyntaxKind::NAMESPACE_EXPR => {
                if let Some(token) = callee
                    .descendants_with_tokens()
                    .filter_map(|element| element.into_token())
                    .filter(|token| token.kind() == SyntaxKind::IDENT)
                    .last()
                {
                    ranges.insert(token.text_range().start().into());
                }
            }
            _ => {}
        }
    }
    ranges
}

fn is_namespace_qualifier(token: &syntax::SyntaxToken) -> bool {
    token.kind() == SyntaxKind::IDENT
        && token
            .parent_ancestors()
            .any(|ancestor| ancestor.kind() == SyntaxKind::NAMESPACE_EXPR)
        && token
            .next_token()
            .is_some_and(|next| matches!(next.kind(), SyntaxKind::COLON2 | SyntaxKind::COLON3))
}

/// The item's type as hover renders it, asked at the definition's own name so
/// the answer is the definition's scheme rather than an expression's type.
fn type_rendering(
    database: &RootDatabase,
    files: ProjectFiles,
    file: SourceFile,
    node: &SyntaxNode,
) -> Option<String> {
    let name = node
        .descendants()
        .find(|descendant| descendant.kind() == SyntaxKind::NAME)?;
    let hover = ide::hover(database, files, file, name.text_range().start())?;
    let rendering = hover.lines.join(" ");
    (!rendering.is_empty()).then_some(rendering)
}

/// The first roxygen line above the definition, as a one-line summary.
/// Roxygen tags are skipped: a card wants the prose, not `@param`.
fn leading_doc(text: &str, start: TextSize) -> Option<String> {
    let mut doc: Option<String> = None;
    for line in text[..usize::from(start)].lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        let Some(body) = trimmed.strip_prefix("#'").or_else(|| {
            trimmed
                .strip_prefix("#:")
                .is_none()
                .then(|| trimmed.strip_prefix('#'))
                .flatten()
        }) else {
            break;
        };
        let body = body.trim();
        if !body.is_empty() && !body.starts_with('@') {
            doc = Some(body.to_owned());
        }
    }
    doc
}

fn line_of(text: &str, offset: TextSize) -> u32 {
    text[..usize::from(offset)].matches('\n').count() as u32
}

fn diagnostic_counts(database: &RootDatabase, file: SourceFile, span: TextRange) -> (u32, u32) {
    let mut errors = 0;
    let mut warnings = 0;
    for diagnostic in file_diagnostics(database, file) {
        if !span.contains_range(diagnostic.range) {
            continue;
        }
        match diagnostic.severity {
            Severity::Error => errors += 1,
            Severity::Warning => warnings += 1,
        }
    }
    (errors, warnings)
}

fn symbol_kind(kind: DocumentSymbolKind) -> &'static str {
    match kind {
        DocumentSymbolKind::Function => "function",
        DocumentSymbolKind::Value => "value",
        DocumentSymbolKind::TypeDefinition => "type",
        DocumentSymbolKind::AliasDefinition => "alias",
        DocumentSymbolKind::S4Class => "s4class",
        DocumentSymbolKind::S4Generic => "s4generic",
        DocumentSymbolKind::S4Method => "s4method",
        DocumentSymbolKind::R6Class => "r6class",
        DocumentSymbolKind::R6Method => "r6method",
        DocumentSymbolKind::R6Field => "r6field",
    }
}

fn item_kind(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Function => "function",
        ItemKind::Value => "value",
        ItemKind::Statement => "statement",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_of(sources: &[(&str, &str)]) -> Index {
        let owned: Vec<(String, String)> = sources
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
            .collect();
        build(&owned, &None, &None)
    }

    #[test]
    fn definitions_become_items_in_path_then_source_order() {
        let index = index_of(&[
            ("R/b.R", "second <- function() 1\nthird <- 2\n"),
            ("R/a.R", "first <- function(x) x\n"),
        ]);
        let names: Vec<&str> = index.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["first", "second", "third"]);
        let kinds: Vec<&str> = index.items.iter().map(|item| item.kind).collect();
        assert_eq!(kinds, ["function", "function", "value"]);
        assert_eq!(index.files[0].path, "R/a.R");
        assert_eq!(index.files[0].items, [0]);
        assert_eq!(index.files[1].items, [1, 2]);
    }

    #[test]
    fn a_cross_file_call_resolves_to_a_reference_and_an_edge() {
        let index = index_of(&[
            ("R/caller.R", "caller <- function() helper(1)\n"),
            ("R/helper.R", "helper <- function(x) x + 1\n"),
        ]);
        let caller = index
            .items
            .iter()
            .position(|item| item.name == "caller")
            .expect("caller is indexed");
        let helper = index
            .items
            .iter()
            .position(|item| item.name == "helper")
            .expect("helper is indexed");
        let [start, length, target] = index.items[caller].references[..] else {
            panic!("expected exactly one resolved reference");
        };
        let code = &index.items[caller].code;
        assert_eq!(
            &code[start as usize..(start + length) as usize],
            "helper",
            "the reference span must cover the name the canvas expands"
        );
        assert_eq!(target as usize, helper);
        assert_eq!(index.edges, [[caller as u32, helper as u32]]);
    }

    #[test]
    fn a_reference_to_a_stub_name_is_not_expandable() {
        let index = index_of(&[("R/a.R", "f <- function(x) nchar(x)\n")]);
        assert!(
            index.items[0].references.is_empty(),
            "`nchar` is declared by the stub corpus, not by the project, so it has no card to open"
        );
        assert!(index.edges.is_empty());
    }

    #[test]
    fn self_recursion_is_a_reference_but_never_an_edge() {
        let index = index_of(&[(
            "R/a.R",
            "loop <- function(n) if (n > 0) loop(n - 1) else 0\n",
        )]);
        assert_eq!(index.items[0].references.len(), 3);
        assert_eq!(index.items[0].references[2], 0);
        assert!(
            index.edges.is_empty(),
            "a self edge would draw a curve from a card to itself"
        );
    }

    #[test]
    fn tokens_classify_the_shapes_the_palette_distinguishes() {
        let index = index_of(&[(
            "R/a.R",
            "#' Add one.\nf <- function(x = 1L) {\n  # note\n  paste(\"a\", x)\n}\n",
        )]);
        let item = &index.items[0];
        let classes: Vec<(&str, u32)> = item
            .tokens
            .chunks_exact(3)
            .map(|token| {
                (
                    &item.code[token[0] as usize..(token[0] + token[1]) as usize],
                    token[2],
                )
            })
            .collect();
        assert!(classes.contains(&("function", TOKEN_KEYWORD)));
        assert!(classes.contains(&("1L", TOKEN_NUMBER)));
        assert!(classes.contains(&("# note", TOKEN_COMMENT)));
        assert!(classes.contains(&("\"a\"", TOKEN_STRING)));
        assert!(classes.contains(&("paste", TOKEN_CALLEE)));
        assert!(classes.contains(&("<-", TOKEN_OPERATOR)));
        assert_eq!(item.doc.as_deref(), Some("Add one."));
    }

    #[test]
    fn diagnostics_land_on_the_item_that_contains_them() {
        let index = index_of(&[(
            "R/a.R",
            "#: fn(x: integer) -> integer\nf <- function(x) x\ng <- function() f(\"text\")\n",
        )]);
        let g = index
            .items
            .iter()
            .find(|item| item.name == "g")
            .expect("g is indexed");
        assert_eq!(g.errors, 1, "passing a string to fn(integer) is an error");
    }
}
