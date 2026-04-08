use {
    crate::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range},
    analysis,
};

pub fn convert_diagnostics(
    diagnostics: impl IntoIterator<Item = analysis::Diagnostic>,
) -> Vec<Diagnostic> {
    diagnostics.into_iter().map(convert_diagnostic).collect()
}

pub fn convert_diagnostic(diagnostic: analysis::Diagnostic) -> Diagnostic {
    Diagnostic {
        range: convert_range(diagnostic.range),
        severity: Some(convert_severity(diagnostic.severity)),
        code: Some(NumberOrString::String(diagnostic.code.to_string())),
        code_description: None,
        source: Some("roughly".into()),
        message: diagnostic.message,
        related_information: None,
        tags: None,
        data: None,
    }
}

fn convert_severity(severity: analysis::Severity) -> DiagnosticSeverity {
    match severity {
        analysis::Severity::Error => DiagnosticSeverity::ERROR,
        analysis::Severity::Warning => DiagnosticSeverity::WARNING,
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
