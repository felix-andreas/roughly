use {
    crate::{
        config::{Case, LintConfig},
        lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range},
    },
    analysis::{self, Analysis, AnalysisPhase, DocumentId},
    std::path::Path,
};

pub fn current_document_diagnostics(
    analysis_state: &mut Analysis,
    path: &Path,
    lint_config: LintConfig,
) -> Vec<Diagnostic> {
    let Some(document_id) = analysis_state.document_id_for_path(path) else {
        return Vec::new();
    };

    analysis::run_lowering(Some(&[document_id]), analysis_state);

    let mut diagnostics = analysis_state
        .document_phase_diagnostics(document_id, &[AnalysisPhase::Lowering])
        .cloned()
        .collect::<Vec<_>>();
    diagnostics.extend(document_lint_diagnostics(analysis_state, document_id, lint_config));
    convert_diagnostics(diagnostics)
}

pub fn saved_document_diagnostics(
    analysis_state: &mut Analysis,
    path: &Path,
    lint_config: LintConfig,
    include_typecheck: bool,
) -> Vec<Diagnostic> {
    let Some(document_id) = analysis_state.document_id_for_path(path) else {
        return Vec::new();
    };

    let mut diagnostics = if include_typecheck {
        analysis::check(analysis_state);
        analysis_state
            .document_phase_diagnostics(
                document_id,
                &[
                    AnalysisPhase::Lowering,
                    AnalysisPhase::Naming,
                    AnalysisPhase::Typecheck,
                ],
            )
            .cloned()
            .collect::<Vec<_>>()
    } else {
        analysis::run_lowering(Some(&[document_id]), analysis_state);
        analysis_state
            .document_phase_diagnostics(document_id, &[AnalysisPhase::Lowering])
            .cloned()
            .collect::<Vec<_>>()
    };

    diagnostics.extend(document_lint_diagnostics(analysis_state, document_id, lint_config));
    convert_diagnostics(diagnostics)
}

fn document_lint_diagnostics(
    analysis_state: &Analysis,
    document_id: DocumentId,
    lint_config: LintConfig,
) -> Vec<analysis::Diagnostic> {
    let Some(document) = analysis_state.document_by_id(document_id) else {
        return Vec::new();
    };

    analysis::lint::analyze(document, convert_lint_config(lint_config))
}

fn convert_lint_config(config: LintConfig) -> analysis::LintConfig {
    analysis::LintConfig {
        naming_style: config.naming_style.map(|naming_style| match naming_style {
            Case::Camel => analysis::NameStyle::Camel,
            Case::Snake => analysis::NameStyle::Snake,
        }),
    }
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
