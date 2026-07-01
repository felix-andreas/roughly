use {
    crate::{
        lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString},
        position::{self, PositionEncoding},
    },
    ropey::Rope,
};

pub fn convert_diagnostics(
    diagnostics: impl IntoIterator<Item = analysis::Diagnostic>,
    rope: &Rope,
    encoding: PositionEncoding,
) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .map(|diagnostic| convert_diagnostic(diagnostic, rope, encoding))
        .collect()
}

pub fn convert_diagnostic(
    diagnostic: analysis::Diagnostic,
    rope: &Rope,
    encoding: PositionEncoding,
) -> Diagnostic {
    Diagnostic {
        range: position::tree_sitter_range_to_lsp(rope, encoding, diagnostic.range),
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
