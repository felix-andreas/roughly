use crate::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};

pub fn diagnostics_for_source(source: &str) -> Vec<Diagnostic> {
    typing::check(source)
        .diagnostics
        .into_iter()
        .map(convert_diagnostic)
        .collect()
}

pub fn analyze_rope(rope: &ropey::Rope) -> Vec<Diagnostic> {
    let source = rope.to_string();
    diagnostics_for_source(&source)
}

fn convert_diagnostic(diagnostic: typing::Diagnostic) -> Diagnostic {
    Diagnostic {
        range: convert_range(diagnostic.range),
        severity: Some(convert_severity(diagnostic.severity)),
        code: Some(NumberOrString::String(diagnostic.code.to_string())),
        code_description: None,
        source: Some("typing".into()),
        message: diagnostic.message,
        related_information: None,
        tags: None,
        data: None,
    }
}

fn convert_severity(severity: typing::Severity) -> DiagnosticSeverity {
    match severity {
        typing::Severity::Error => DiagnosticSeverity::ERROR,
    }
}

fn convert_range(range: tree_sitter::Range) -> Range {
    Range::new(
        Position::new(
            range.start_point.row as u32,
            range.start_point.column as u32,
        ),
        Position::new(range.end_point.row as u32, range.end_point.column as u32),
    )
}
