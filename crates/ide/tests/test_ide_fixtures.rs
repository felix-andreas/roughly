//! IDE feature fixtures: the case source carries one `$0` cursor marker
//! (stripped before analysis); the expectation renders each feature's result
//! at that position. `ROUGHLY_BLESS=1` accepts new output;
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

fn render_at(
    db: &RootDatabase,
    files: ProjectFiles,
    file: SourceFile,
    offset: TextSize,
    output: &mut String,
) {
    match ide::hover(db, file, offset) {
        Some(hover) => {
            for line in &hover.lines {
                output.push_str(&format!(
                    "hover {}..{}: {line}\n",
                    u32::from(hover.range.start()),
                    u32::from(hover.range.end()),
                ));
            }
        }
        None => output.push_str("hover: none\n"),
    }
    match ide::definition(db, files, file, offset) {
        Some(target) => output.push_str(&format!(
            "definition {}..{}\n",
            u32::from(target.range.start()),
            u32::from(target.range.end()),
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
    match ide::signature_help(db, file, offset) {
        Some(help) => {
            output.push_str(&format!("signature: {}\n", help.label));
            let parameters: Vec<String> = help
                .parameters
                .iter()
                .enumerate()
                .map(|(index, span)| {
                    let text = &help.label[usize::from(span.start())..usize::from(span.end())];
                    if Some(index) == help.active_parameter {
                        format!("[{text}]")
                    } else {
                        text.to_owned()
                    }
                })
                .collect();
            output.push_str(&format!("parameters: {}\n", parameters.join(" | ")));
        }
        None => output.push_str("signature: none\n"),
    }
}

#[test]
fn ide_fixtures() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ide");
    syntax::testing::run_fixture_suite(&suite, &render);
}
