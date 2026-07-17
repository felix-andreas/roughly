//! Semantic pins for the interface-SCC fixed point: forward references through the interface
//! resolve to the definition's real scheme (not a tolerant `Unknown`), whichever round
//! granularity — whole-file or per-definition — the member files take.

use engine::{
    Engine,
    queries::{Config, FileDiagnostics, Key, RoughlyQueries, source_input},
};

// A forward call of a same-file integer binding is a self-edge SCC; its converged scheme must be
// the integer, so the call errors instead of silently passing through `Unknown`.
#[test]
fn forward_reference_to_integer_errors() {
    let source =
        "#: @alias Count {integer}\n\nalpha <- delta(\"s\")\ndelta <- 1L\nbeta <- nchar(1L)\n";
    let mut engine = Engine::new(RoughlyQueries::new());
    engine.set_input(Key::ProjectFiles, vec![0u32]);
    engine.set_input(
        Key::Config,
        Config {
            check: analysis::CheckConfig {
                typing: true,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    engine.set_input(Key::SourceText(0), source_input(source));
    engine.set_input(
        Key::DocumentKind(0),
        analysis::naming::DocumentKind::Package,
    );
    engine.set_input(Key::FileName(0), std::path::PathBuf::from("/ws/s0000.R"));
    let diagnostics = engine.fetch::<FileDiagnostics>(Key::Diagnostics(0));
    assert!(
        !diagnostics.type_errors.is_empty(),
        "expected the forward call of an integer to error, got none"
    );
}
