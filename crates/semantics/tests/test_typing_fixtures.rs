//! Typing fixture suite: each case runs the full semantic pipeline on one
//! package file (shipped stubs installed) and renders every named top-level
//! definition's exported scheme followed by the file's diagnostics.
//! `ROUGHLY_BLESS=1` accepts new output; `FIXTURE_FILTER=group__case` runs one
//! case.

use semantics::diagnostics::{Severity, TypeRenderer, file_diagnostics};
use semantics::{
    DocumentKind, ItemKind, ProjectFiles, RootDatabase, SourceFile, item_check, item_tree,
};
use std::path::Path;

fn render(source: &str) -> String {
    render_as(source, DocumentKind::Package)
}

fn render_script(source: &str) -> String {
    render_as(source, DocumentKind::Script)
}

fn render_as(source: &str, kind: DocumentKind) -> String {
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let file = SourceFile::new(&db, source.to_owned(), kind);
    ProjectFiles::new(&db, vec![file]);

    let mut output = String::new();
    for item in item_tree(&db, file) {
        if !matches!(*item.kind(&db), ItemKind::Function | ItemKind::Value) {
            continue;
        }
        let Some(name) = item.name(&db).clone() else {
            continue;
        };
        let Some(check) = item_check(&db, item) else {
            continue;
        };
        let Some(scheme) = check.scheme else {
            continue;
        };
        let mut renderer = TypeRenderer::default();
        output.push_str(&name);
        output.push_str(": ");
        output.push_str(&renderer.render_scheme(&db, &scheme));
        output.push('\n');
    }
    for diagnostic in file_diagnostics(&db, file) {
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        output.push_str(&format!(
            "{}..{} {severity}[{}] {}\n",
            u32::from(diagnostic.range.start()),
            u32::from(diagnostic.range.end()),
            diagnostic.code,
            diagnostic.message
        ));
    }
    output
}

#[test]
fn typing_fixtures() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/typing");
    syntax::testing::run_fixture_suite(&suite, &render);
}

/// The same pipeline over script documents: one sequential top-down scope.
#[test]
fn typing_script_fixtures() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/typing-scripts");
    syntax::testing::run_fixture_suite(&suite, &render_script);
}
