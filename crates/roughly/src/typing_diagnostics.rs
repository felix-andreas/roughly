use {
    crate::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range},
    ropey::Rope,
};

pub fn analyze(
    _node: tree_sitter::Node,
    rope: &Rope,
    analysis_state: &mut analysis::Analysis,
) -> Vec<Diagnostic> {
    let document_path = analysis_state.base_path().join("R").join("current.R");

    if analysis_state
        .add_document_from_source(document_path, &rope.to_string())
        .is_err()
    {
        return Vec::new();
    }

    convert_diagnostics(analysis::check(analysis_state).diagnostics)
}

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
        source: Some("typing".into()),
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
