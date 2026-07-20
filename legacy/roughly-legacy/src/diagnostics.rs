use {
    crate::{
        lsp_types::{Diagnostic, DiagnosticSeverity, DiagnosticTag, NumberOrString, Range},
        position::{self, PositionEncoding},
    },
    analysis::{hir::TypingMode, text::TextPosition},
    engine::{
        Engine,
        queries::{Config as EngineConfig, FileDiagnostics, FileId, Key, RoughlyQueries},
    },
    ropey::Rope,
};

/// The final diagnostic set for one engine file: every class from [`FileDiagnostics`], gated by
/// config exactly as production's `document_diagnostics` gates them, with type errors rendered
/// against the engine's interner + fallback range and strict escalation applied. Shared by the
/// language server's publish path and `roughly check`, so the two surfaces can never drift.
/// Suppression comments are the caller's step (the server folds them into its rendering tail; the
/// CLI applies them against the source it read).
pub fn assemble_engine_file_diagnostics(
    engine: &Engine<RoughlyQueries>,
    file: FileId,
) -> Vec<analysis::Diagnostic> {
    let file_diagnostics = engine.fetch::<FileDiagnostics>(Key::Diagnostics(file));
    let fallback = *engine.fetch::<tree_sitter::Range>(Key::FallbackRange);
    let config = engine.fetch::<EngineConfig>(Key::Config);

    let mut rendered = Vec::new();
    // Local naming, package naming, lowering (syntax), and lint are emitted unconditionally,
    // exactly as production's `document_diagnostics` does (they are not config-gated).
    rendered.extend(file_diagnostics.naming.iter().cloned());
    rendered.extend(file_diagnostics.package_naming.iter().cloned());
    rendered.extend(file_diagnostics.lowering.iter().cloned());
    rendered.extend(file_diagnostics.lint.iter().cloned());
    if config.check.unused {
        rendered.extend(file_diagnostics.unused.iter().cloned());
    }
    let (typing_enabled, strict_enabled) = TypingMode::effective(
        file_diagnostics.typing_override,
        config.check.typing,
        config.check.strict,
    );
    if typing_enabled {
        engine.group().with_interner(|interner| {
            for error in &file_diagnostics.type_errors {
                rendered.push(analysis::Diagnostic::from_inference_error(
                    error, fallback, interner,
                ));
            }
        });
    }
    if strict_enabled {
        rendered.extend(file_diagnostics.strict_diagnostics.iter().cloned());
        for diagnostic in &mut rendered {
            diagnostic.escalate_unresolved_to_error();
        }
    }
    rendered
}

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
        || diagnostic.code == analysis::DiagnosticCode::Lint(analysis::Lint::UnusedParameter)
        || diagnostic.code == analysis::DiagnosticCode::Lint(analysis::Lint::UnusedImport))
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
