//! IDE feature fixtures: the case source carries one `$0` cursor marker
//! (stripped before analysis); the expectation renders each feature's result
//! at that position. `ROUGHLY_BLESS=1` accepts new output;
//! `FIXTURE_FILTER=group__case` runs one case.

use semantics::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};
use std::path::Path;
use syntax::TextSize;

fn split_marker(source: &str) -> (String, TextSize) {
    let at = source
        .find("$0")
        .expect("fixture source must carry a $0 cursor marker");
    let mut text = source.to_owned();
    text.replace_range(at..at + 2, "");
    (text, TextSize::from(at as u32))
}

fn render(source: &str) -> String {
    let (text, offset) = split_marker(source);
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let file = SourceFile::new(&db, text, DocumentKind::Package);
    let files = ProjectFiles::new(&db, vec![file]);

    let mut output = String::new();
    match ide::hover(&db, file, offset) {
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
    match ide::definition(&db, files, file, offset) {
        Some(target) => output.push_str(&format!(
            "definition {}..{}\n",
            u32::from(target.range.start()),
            u32::from(target.range.end()),
        )),
        None => output.push_str("definition: none\n"),
    }
    let references = ide::references(&db, file, offset);
    if references.is_empty() {
        output.push_str("references: none\n");
    } else {
        let ranges: Vec<String> = references
            .iter()
            .map(|target| {
                format!(
                    "{}..{}",
                    u32::from(target.range.start()),
                    u32::from(target.range.end())
                )
            })
            .collect();
        output.push_str(&format!("references: {}\n", ranges.join(", ")));
    }
    output
}

#[test]
fn ide_fixtures() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ide");
    syntax::testing::run_fixture_suite(&suite, &render);
}
