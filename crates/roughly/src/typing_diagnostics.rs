use {
    crate::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range},
    ropey::Rope,
    std::path::PathBuf,
};

pub fn analyze(
    _node: tree_sitter::Node,
    rope: &Rope,
    analysis_state: &mut typing::Analysis,
) -> Vec<Diagnostic> {
    let document_path = analysis_state.base_path().join("R").join("current.R");

    if analysis_state
        .add_document_from_source(document_path, &rope.to_string())
        .is_err()
    {
        return Vec::new();
    }

    typing::check(analysis_state)
        .diagnostics
        .into_iter()
        .map(convert_diagnostic)
        .collect()
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
