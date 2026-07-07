use {
    crate::{
        lsp_types::{Diagnostic, DiagnosticSeverity, DiagnosticTag, NumberOrString, Range},
        position::{self, PositionEncoding},
    },
    analysis::text::TextPosition,
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
    // The unnecessary tag lets editors render dead code faded instead of (or alongside) squiggled —
    // the conventional presentation for an assignment whose value is never read or a parameter no
    // read uses.
    let tags = (diagnostic.code == analysis::DiagnosticCode::Unused
        || diagnostic.code == analysis::DiagnosticCode::Lint(analysis::Lint::UnusedParameter))
    .then(|| vec![DiagnosticTag::UNNECESSARY]);
    Diagnostic {
        range: position::tree_sitter_range_to_lsp(rope, encoding, diagnostic.range),
        severity: Some(convert_severity(diagnostic.severity)),
        code: Some(NumberOrString::String(diagnostic.code.to_string())),
        code_description: None,
        source: Some("roughly".into()),
        message: diagnostic.message,
        related_information: None,
        tags,
        data: None,
    }
}

fn convert_severity(severity: analysis::Severity) -> DiagnosticSeverity {
    match severity {
        analysis::Severity::Error => DiagnosticSeverity::ERROR,
        analysis::Severity::Warning => DiagnosticSeverity::WARNING,
    }
}

// A whole-line error diagnostic for one declaration the stub loader drops, shared by the server's
// `.Rtypes` buffer diagnostics and the CLI's override report so both surfaces render the problem
// identically.
pub fn convert_stub_problem(
    problem: &analysis::stdlib::StubProblem,
    rope: &Rope,
    encoding: PositionEncoding,
) -> Diagnostic {
    let line_length = rope
        .get_line(problem.line)
        .map(|line| {
            let mut length = line.len_chars();
            while length > 0 && matches!(line.char(length - 1), '\n' | '\r') {
                length -= 1;
            }
            length
        })
        .unwrap_or(0);
    let start = position::internal_position_to_lsp(
        rope,
        encoding,
        TextPosition {
            line_index: problem.line,
            character_index: 0,
        },
    );
    let end = position::internal_position_to_lsp(
        rope,
        encoding,
        TextPosition {
            line_index: problem.line,
            character_index: line_length,
        },
    );
    Diagnostic {
        range: Range { start, end },
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String("stub".to_owned())),
        code_description: None,
        source: Some("roughly".into()),
        message: problem.message.clone(),
        related_information: None,
        tags: None,
        data: None,
    }
}
