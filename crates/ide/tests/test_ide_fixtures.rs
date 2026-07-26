//! IDE feature fixtures: the case source carries one `$0` cursor marker
//! (stripped before analysis); the expectation renders each feature's result
//! at that position. `RY_BLESS=1` accepts new output;
//! `FIXTURE_FILTER=group__case` runs one case.

use semantics::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};
use std::path::Path;
use syntax::TextSize;

/// The `$0` cursor marker, stripped from the source. Cases without a marker
/// render only the position-independent features (inlay hints).
fn split_marker(source: &str) -> (String, Option<TextSize>) {
    let Some(at) = source.find("$0") else {
        return (source.to_owned(), None);
    };
    let mut text = source.to_owned();
    text.replace_range(at..at + 2, "");
    (text, Some(TextSize::from(at as u32)))
}

fn render(source: &str) -> String {
    let (text, offset) = split_marker(source);
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let file = SourceFile::new(&db, text, DocumentKind::Package);
    let files = ProjectFiles::new(&db, vec![file]);

    let mut output = String::new();
    if let Some(offset) = offset {
        render_at(&db, files, file, offset, &mut output);
    }
    for hint in ide::inlay_hints(&db, file, None) {
        output.push_str(&format!("hint @{}{}\n", u32::from(hint.offset), hint.label));
    }
    output
}

/// The text a stub-source range covers, rendered instead of the absolute byte
/// offsets. A declaration's offset into the shipped corpus shifts whenever an
/// unrelated stub is added, so pinning the number made every corpus addition
/// re-bless these cases while proving nothing; pinning the *token* proves the
/// range points where it should and survives.
fn stub_text(db: &RootDatabase, source_index: usize, range: syntax::TextRange) -> String {
    let Some(sources) = semantics::stubs::StubSources::try_get(db) else {
        return "<no stub sources>".to_owned();
    };
    let Some((_, text)) = sources.sources(db).get(source_index) else {
        return "<no such stub source>".to_owned();
    };
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    match text.get(start..end) {
        Some(slice) => format!("`{slice}`"),
        None => "<range outside the stub source>".to_owned(),
    }
}

fn render_at(
    db: &RootDatabase,
    files: ProjectFiles,
    file: SourceFile,
    offset: TextSize,
    output: &mut String,
) {
    match ide::hover(db, files, file, offset) {
        Some(hover) => {
            for line in &hover.lines {
                output.push_str(&format!(
                    "hover {}..{}: {line}\n",
                    u32::from(hover.range.start()),
                    u32::from(hover.range.end()),
                ));
            }
            match hover.definition {
                Some(ide::HoverDefinition::Local {
                    target,
                    maybe_undefined,
                }) => output.push_str(&format!(
                    "hover-definition: local {}..{}{}\n",
                    u32::from(target.range.start()),
                    u32::from(target.range.end()),
                    if maybe_undefined {
                        " (maybe undefined)"
                    } else {
                        ""
                    },
                )),
                Some(ide::HoverDefinition::Global { target }) => output.push_str(&format!(
                    "hover-definition: global {}..{}\n",
                    u32::from(target.range.start()),
                    u32::from(target.range.end()),
                )),
                Some(ide::HoverDefinition::Stub {
                    namespace,
                    overloads,
                    declaration,
                }) => {
                    let declared = declaration
                        .map(|target| {
                            format!(
                                ", declared in stub source {} at {}",
                                target.source_index,
                                stub_text(db, target.source_index, target.range),
                            )
                        })
                        .unwrap_or_default();
                    output.push_str(&format!(
                        "hover-definition: package {namespace} ({overloads} declaration(s){declared})\n"
                    ))
                }
                None => {}
            }
        }
        None => output.push_str("hover: none\n"),
    }
    match ide::definition(db, files, file, offset) {
        Some(ide::DefinitionTarget::Project(target)) => output.push_str(&format!(
            "definition {}..{}\n",
            u32::from(target.range.start()),
            u32::from(target.range.end()),
        )),
        Some(ide::DefinitionTarget::Stub(target)) => output.push_str(&format!(
            "definition: stub source {} at {}\n",
            target.source_index,
            stub_text(db, target.source_index, target.range),
        )),
        None => output.push_str("definition: none\n"),
    }
    let references = ide::references(db, files, file, offset, true);
    if references.is_empty() {
        output.push_str("references: none\n");
    } else {
        let ranges: Vec<String> = references
            .iter()
            .map(|occurrence| {
                format!(
                    "{}..{}{}",
                    u32::from(occurrence.range.start()),
                    u32::from(occurrence.range.end()),
                    if occurrence.is_declaration { "*" } else { "" }
                )
            })
            .collect();
        output.push_str(&format!("references: {}\n", ranges.join(", ")));
    }
    match ide::rename(db, files, file, offset) {
        Some(edits) => output.push_str(&format!("rename: {} edit(s)\n", edits.len())),
        None => output.push_str("rename: none\n"),
    }
    match ide::type_definition(db, files, file, offset) {
        Some(ide::DefinitionTarget::Project(target)) => output.push_str(&format!(
            "type-definition {}..{}\n",
            u32::from(target.range.start()),
            u32::from(target.range.end()),
        )),
        Some(ide::DefinitionTarget::Stub(target)) => output.push_str(&format!(
            "type-definition: stub source {} at {}\n",
            target.source_index,
            stub_text(db, target.source_index, target.range),
        )),
        None => {}
    }
    match ide::signature_help(db, file, offset) {
        Some(help) => {
            for (signature_index, signature) in help.signatures.iter().enumerate() {
                let marker =
                    if help.signatures.len() > 1 && signature_index == help.active_signature {
                        " (active)"
                    } else {
                        ""
                    };
                output.push_str(&format!("signature: {}{marker}\n", signature.label));
                let parameters: Vec<String> = signature
                    .parameters
                    .iter()
                    .enumerate()
                    .map(|(index, span)| {
                        let text =
                            &signature.label[usize::from(span.start())..usize::from(span.end())];
                        if Some(index) == signature.active_parameter {
                            format!("[{text}]")
                        } else {
                            text.to_owned()
                        }
                    })
                    .collect();
                output.push_str(&format!("parameters: {}\n", parameters.join(" | ")));
            }
        }
        None => output.push_str("signature: none\n"),
    }
    for action in ide::code_actions(db, file, syntax::TextRange::new(offset, offset)) {
        output.push_str(&format!("action: {}\n", action.title));
        for edit in &action.edits {
            output.push_str(&format!(
                "  edit {}..{} -> {:?}\n",
                u32::from(edit.range.start()),
                u32::from(edit.range.end()),
                edit.replacement
            ));
        }
    }
    match ide::completion(db, files, file, offset) {
        Some(result) => {
            let labels: Vec<String> = result
                .items
                .iter()
                .take(8)
                .map(|item| item.label.clone())
                .collect();
            output.push_str(&format!(
                "completion ({}{}): {}\n",
                result.items.len(),
                if result.is_incomplete { "+" } else { "" },
                labels.join(", ")
            ));
        }
        None => output.push_str("completion: none\n"),
    }
}

/// Workspace symbols rank by the shared matcher across every project file.
#[test]
fn workspace_symbols_ranked() {
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let first = SourceFile::new(
        &db,
        "alpha_one <- function() 1\nbeta <- 2\n".to_owned(),
        DocumentKind::Package,
    );
    let second = SourceFile::new(
        &db,
        "alpha_two <- function() 3\nnot_matching <- 4\n".to_owned(),
        DocumentKind::Package,
    );
    let files = ProjectFiles::new(&db, vec![first, second]);

    let names: Vec<String> = ide::workspace_symbols(&db, files, "alpha")
        .into_iter()
        .map(|symbol| symbol.name)
        .collect();
    assert_eq!(names, vec!["alpha_one".to_owned(), "alpha_two".to_owned()]);

    let subsequence: Vec<String> = ide::workspace_symbols(&db, files, "aone")
        .into_iter()
        .map(|symbol| symbol.name)
        .collect();
    assert_eq!(subsequence, vec!["alpha_one".to_owned()]);
}

/// S4 declarations and R6 classes outline with their own kinds; R6 members
/// nest as children (methods for function values, fields otherwise, active
/// bindings as fields).
#[test]
fn document_symbols_expose_s4_and_r6_hierarchy() {
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let source = "\
setClass(\"Person\", representation(name = \"character\"))
setGeneric(\"greet\", function(object) standardGeneric(\"greet\"))
setMethod(\"greet\", \"Person\", function(object) object@name)
Account <- R6Class(\"Account\",
  public = list(
    balance = 0,
    deposit = function(amount) invisible(self)
  ),
  private = list(audit = function() NULL),
  active = list(status = function() \"open\")
)
plain <- function(x, ...) x
";
    let file = SourceFile::new(&db, source.to_owned(), DocumentKind::Package);
    ProjectFiles::new(&db, vec![file]);

    let symbols = ide::document_symbols(&db, file);
    let rendered: Vec<String> = symbols
        .iter()
        .map(|symbol| {
            format!(
                "{} ({:?}{})",
                symbol.name,
                symbol.kind,
                symbol
                    .detail
                    .as_deref()
                    .map(|detail| format!(", {detail}"))
                    .unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(
        rendered,
        vec![
            "Person (S4Class)",
            "greet (S4Generic)",
            "greet (S4Method, Person)",
            "Account (R6Class)",
            "plain (Function, fn(x, ...))",
        ],
        "{symbols:#?}"
    );

    let account = &symbols[3];
    let members: Vec<String> = account
        .children
        .iter()
        .map(|child| format!("{} ({:?})", child.name, child.kind))
        .collect();
    assert_eq!(
        members,
        vec![
            "balance (R6Field)",
            "deposit (R6Method)",
            "audit (R6Method)",
            "status (R6Field)",
        ],
        "{account:#?}"
    );

    // R6 members are reachable through workspace symbols.
    let files = semantics::ProjectFiles::try_get(&db).expect("project files installed");
    let deposit: Vec<String> = ide::workspace_symbols(&db, files, "deposit")
        .into_iter()
        .map(|symbol| symbol.name)
        .collect();
    assert_eq!(deposit, vec!["deposit".to_owned()]);
}

#[test]
fn ide_fixtures() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ide");
    syntax::testing::run_fixture_suite(&suite, &render);
}
