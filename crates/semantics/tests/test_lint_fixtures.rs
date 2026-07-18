//! Lint fixture suite: each case runs `lints::lint_file` on one package file
//! and renders every finding. The `lints` directory runs the default config;
//! the `lints-style` directory opts into `naming-style = "snake_case"` and
//! `unused-parameter = "warn"` (both off by default). `ROUGHLY_BLESS=1`
//! accepts new output; `FIXTURE_FILTER=group__case` runs one case.

use semantics::diagnostics::Severity;
use semantics::lints::{LintConfig, LintLevel, NameStyle, lint_file};
use semantics::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};
use std::path::Path;

fn render_with(source: &str, config: &LintConfig) -> String {
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let file = SourceFile::new(&db, source.to_owned(), DocumentKind::Package);
    ProjectFiles::new(&db, vec![file]);

    let mut output = String::new();
    for diagnostic in lint_file(&db, file, config) {
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
    if output.is_empty() {
        output.push_str("clean\n");
    }
    output
}

#[test]
fn lint_fixtures() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lints");
    syntax::testing::run_fixture_suite(&suite, &|source| {
        render_with(source, &LintConfig::default())
    });
}

#[test]
fn lint_style_fixtures() {
    let config = LintConfig {
        naming_style: Some(NameStyle::Snake),
        unused_parameter: LintLevel::Warn,
        ..LintConfig::default()
    };
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lints-style");
    syntax::testing::run_fixture_suite(&suite, &|source| render_with(source, &config));
}
