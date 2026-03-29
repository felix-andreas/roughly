use {
    crate::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range},
    ropey::Rope,
    std::path::{Path, PathBuf},
};

pub fn analyze(
    _node: tree_sitter::Node,
    rope: &Rope,
    analysis_state: &mut typing::AnalysisState,
) -> Vec<Diagnostic> {
    let mut workspace = match typing::Workspace::new() {
        Ok(workspace) => workspace,
        Err(_) => return Vec::new(),
    };
    let package_path = PathBuf::from("/typing");
    let document_path = PathBuf::from("/typing/current.R");

    if workspace.insert_package(package_path.clone()).is_err() {
        return Vec::new();
    }
    if workspace
        .insert_package_document(&package_path, document_path, &rope.to_string())
        .is_err()
    {
        return Vec::new();
    }

    let Some(package) = workspace.package(Path::new("/typing")) else {
        return Vec::new();
    };

    typing::check(package, analysis_state)
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
