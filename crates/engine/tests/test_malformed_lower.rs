// The engine's `Lower` query must lower a file whose tree carries any error node to an *empty* `Module`,
// so a transiently-malformed keystroke emits no naming/type/strict diagnostics — matching what the
// `analysis` from-scratch path suppresses off a broken error-recovery tree. The differential oracle
// cannot guard this: its edit generator only produces well-formed input, so a partial-tree divergence
// would never appear in a generated stream. These cases pin the invariant directly.
//
// Each malformed case is paired with a well-formed control that DOES produce the diagnostic, so the
// assertions are tight: removing the `!root_node().has_error()` short-circuit in the `Lower` query lowers
// the error-recovery HIR instead, the control's diagnostic reappears on the malformed input, and the
// corresponding empty-diagnostics assertion fails.

use engine::{
    Engine,
    queries::{Config, FileDiagnostics, FileId, Key, RoughlyQueries, parse_source_input},
};

const FILE: FileId = 0;

fn diagnostics_for(source: &str) -> std::rc::Rc<FileDiagnostics> {
    let mut engine = Engine::new(RoughlyQueries::new());
    engine.set_input(Key::ProjectFiles, vec![FILE]);
    engine.set_input(
        Key::Config,
        Config {
            check: analysis::CheckConfig {
                typing: true,
                strict: false,
                unused: false,
            },
            lint: Default::default(),
        },
    );
    engine.set_input(Key::SourceText(FILE), parse_source_input(source));
    engine.set_input(
        Key::DocumentKind(FILE),
        analysis::naming::DocumentKind::Package,
    );
    engine.fetch::<FileDiagnostics>(Key::Diagnostics(FILE))
}

fn assert_no_semantic_diagnostics(source: &str) {
    let diagnostics = diagnostics_for(source);
    assert!(
        diagnostics.naming.is_empty()
            && diagnostics.package_naming.is_empty()
            && diagnostics.type_errors.is_empty()
            && diagnostics.strict_diagnostics.is_empty()
            && diagnostics.unused.is_empty(),
        "malformed input must lower to an empty module and emit no semantic diagnostics, but got \
         naming={} package_naming={} type_errors={} strict={} unused={}",
        diagnostics.naming.len(),
        diagnostics.package_naming.len(),
        diagnostics.type_errors.len(),
        diagnostics.strict_diagnostics.len(),
        diagnostics.unused.len(),
    );
}

// An unclosed call to an undefined name. The well-formed form resolves the reference and fails, so a
// naming diagnostic is the control signal; the malformed form must produce none.
#[test]
fn unclosed_call_to_undefined_name_emits_no_naming_diagnostic() {
    let control = diagnostics_for("y <- undefined_fn(1L)\n");
    assert_eq!(
        control.package_naming.len(),
        1,
        "control: a well-formed reference to an undefined name is an unresolved-name diagnostic"
    );

    assert_no_semantic_diagnostics("y <- undefined_fn(1L\n");
}

// A type-mismatched call left unclosed. The well-formed form type-checks and fails; the malformed form
// must produce no type error.
#[test]
fn unclosed_type_mismatch_emits_no_type_error() {
    let control = diagnostics_for("#: fn(x: integer) -> integer\nf <- function(x) x\nf(\"s\")\n");
    assert_eq!(
        control.type_errors.len(),
        1,
        "control: a well-formed argument-type mismatch is a type error"
    );

    assert_no_semantic_diagnostics("#: fn(x: integer) -> integer\nf <- function(x) x\nf(\"s\"\n");
}

// A dangling assignment (`x <- ` with no right-hand side) is the canonical transiently-broken keystroke.
#[test]
fn dangling_assignment_emits_no_semantic_diagnostic() {
    assert_no_semantic_diagnostics("x <- \n");
}
