use analysis::{
    typecheck::{InferenceEntry, InferenceState},
    types::{Atomic, Constraint, CoreType},
};

fn unbound() -> InferenceEntry {
    InferenceEntry::Unbound {
        level: 0,
        constraint: Constraint::Unconstrained,
    }
}

#[test]
fn fresh_variables_start_unbound() {
    let mut inference_state = InferenceState::new();

    let variable = inference_state.fresh_variable();

    assert_eq!(
        inference_state.entry(variable),
        Some(&InferenceEntry::Unbound {
            level: 0,
            constraint: analysis::types::Constraint::Unconstrained
        })
    );
}

#[test]
fn unifying_two_variables_creates_a_redirect() {
    let mut inference_state = InferenceState::new();
    let left = inference_state.fresh_variable();
    let right = inference_state.fresh_variable();

    let unified_type = inference_state
        .unify(CoreType::Variable(left), CoreType::Variable(right))
        .expect("variables should unify");

    assert_eq!(unified_type, CoreType::Variable(right));
    assert_eq!(
        inference_state.entry(left),
        Some(&InferenceEntry::Redirect(right))
    );
}

#[test]
fn resolving_a_redirected_variable_applies_path_compression() {
    let mut inference_state = InferenceState::new();
    let first = inference_state.fresh_variable();
    let second = inference_state.fresh_variable();
    let third = inference_state.fresh_variable();

    inference_state
        .unify(CoreType::Variable(first), CoreType::Variable(second))
        .expect("first and second should unify");
    inference_state
        .unify(CoreType::Variable(second), CoreType::Variable(third))
        .expect("second and third should unify");
    inference_state
        .unify(CoreType::Variable(third), CoreType::Scalar(Atomic::Integer))
        .expect("third should unify with integer");

    let resolved_type = inference_state
        .resolve(CoreType::Variable(first))
        .expect("first variable should resolve");

    assert_eq!(resolved_type, CoreType::Scalar(Atomic::Integer));
    assert_eq!(
        inference_state.entry(first),
        Some(&InferenceEntry::Bound(CoreType::Scalar(Atomic::Integer)))
    );
}

#[test]
fn rollback_restores_a_bound_variable_and_reclaims_fresh_ids() {
    let mut inference_state = InferenceState::new();

    let snapshot = inference_state.snapshot();
    let variable = inference_state.fresh_variable();
    inference_state
        .unify(
            CoreType::Variable(variable),
            CoreType::Scalar(Atomic::Integer),
        )
        .expect("variable should bind to integer");
    assert_eq!(
        inference_state.entry(variable),
        Some(&InferenceEntry::Bound(CoreType::Scalar(Atomic::Integer)))
    );

    inference_state.rollback_to(snapshot);

    // The variable allocated inside the snapshot is gone entirely (entry removed, id reclaimed).
    assert_eq!(inference_state.entry(variable), None);
    let reused = inference_state.fresh_variable();
    assert_eq!(
        reused, variable,
        "the reclaimed id should be handed out again"
    );
    assert_eq!(inference_state.entry(reused), Some(&unbound()));
}

#[test]
fn rollback_restores_two_unified_variables_to_independent_unbound() {
    let mut inference_state = InferenceState::new();
    let left = inference_state.fresh_variable();
    let right = inference_state.fresh_variable();

    let snapshot = inference_state.snapshot();
    inference_state
        .unify(CoreType::Variable(left), CoreType::Variable(right))
        .expect("variables should unify");
    assert_eq!(
        inference_state.entry(left),
        Some(&InferenceEntry::Redirect(right))
    );

    inference_state.rollback_to(snapshot);

    assert_eq!(inference_state.entry(left), Some(&unbound()));
    assert_eq!(inference_state.entry(right), Some(&unbound()));
}

#[test]
fn nested_snapshots_compose_and_commit_keeps_mutations() {
    let mut inference_state = InferenceState::new();
    let outer_variable = inference_state.fresh_variable();

    let outer = inference_state.snapshot();
    inference_state
        .unify(
            CoreType::Variable(outer_variable),
            CoreType::Scalar(Atomic::Integer),
        )
        .expect("outer variable should bind");
    let inner_variable = inference_state.fresh_variable();

    let inner = inference_state.snapshot();
    inference_state
        .unify(
            CoreType::Variable(inner_variable),
            CoreType::Scalar(Atomic::Logical),
        )
        .expect("inner variable should bind");

    // Inner rollback reverses only the inner write; the outer binding survives.
    inference_state.rollback_to(inner);
    assert_eq!(
        inference_state.entry(outer_variable),
        Some(&InferenceEntry::Bound(CoreType::Scalar(Atomic::Integer)))
    );
    assert_eq!(inference_state.entry(inner_variable), Some(&unbound()));

    // Committing the outer region keeps every surviving mutation.
    inference_state.commit(outer);
    assert_eq!(
        inference_state.entry(outer_variable),
        Some(&InferenceEntry::Bound(CoreType::Scalar(Atomic::Integer)))
    );

    // After the outermost commit the log is empty again: a fresh snapshot/rollback reverses only its
    // own writes and leaves the committed binding untouched.
    let after_commit = inference_state.snapshot();
    let probe_variable = inference_state.fresh_variable();
    inference_state
        .unify(
            CoreType::Variable(probe_variable),
            CoreType::Scalar(Atomic::Character),
        )
        .expect("probe variable should bind");
    inference_state.rollback_to(after_commit);
    assert_eq!(inference_state.entry(probe_variable), None);
    assert_eq!(
        inference_state.entry(outer_variable),
        Some(&InferenceEntry::Bound(CoreType::Scalar(Atomic::Integer)))
    );
}

#[test]
fn mutations_without_an_active_snapshot_are_permanent() {
    let mut inference_state = InferenceState::new();
    let permanent = inference_state.fresh_variable();
    inference_state
        .unify(
            CoreType::Variable(permanent),
            CoreType::Scalar(Atomic::Integer),
        )
        .expect("permanent variable should bind");

    // With nothing recorded on the committed path, a later snapshot/rollback cannot reach this write.
    let snapshot = inference_state.snapshot();
    let speculative = inference_state.fresh_variable();
    inference_state
        .unify(
            CoreType::Variable(speculative),
            CoreType::Scalar(Atomic::Logical),
        )
        .expect("speculative variable should bind");
    inference_state.rollback_to(snapshot);

    assert_eq!(
        inference_state.entry(permanent),
        Some(&InferenceEntry::Bound(CoreType::Scalar(Atomic::Integer)))
    );
    assert_eq!(inference_state.entry(speculative), None);
}

// Overload sets declared by repeating a name in a stub source: a call commits the first candidate
// that accepts the arguments, and a call no candidate accepts reports every candidate as tried.
// The shipped corpus always ends a set with an `Any` fallback, so the no-match error is only
// reachable through a project override — which is why this lives here and not in a fixture.
mod overload_sets {
    use analysis::{
        Document,
        diagnostic::Diagnostic,
        lower::{self, LoweringContext},
        stdlib::StubLibrary,
        tree::new_parser,
        typecheck::{TypeDefinitionEnvironment, inference_state_with_builtins},
        types::{Atomic, CoreType},
    };

    const OVERRIDE_SOURCE: &str =
        "wrap : fn(x: integer) -> integer\nwrap : fn(x: character) -> character\n";

    fn infer_last_type(source: &str) -> Result<CoreType, String> {
        let mut parser = new_parser().expect("parser should build");
        let document = Document::parse(&mut parser, source).expect("source should parse");
        let mut lowering_context = LoweringContext::new();
        let module = lower::lower(&document, &mut lowering_context);
        let mut inference_state = inference_state_with_builtins(&mut lowering_context);
        let stub_library = StubLibrary::load_with_overrides(
            lowering_context.interner_mut(),
            &[OVERRIDE_SOURCE.to_owned()],
        );
        stub_library.seed_into(&mut inference_state);
        let type_definitions = TypeDefinitionEnvironment::from_module(&module);
        match inference_state.infer_module(&module, &type_definitions) {
            Ok(mut inferred_types) => {
                let last = inferred_types
                    .pop()
                    .expect("module should have a statement");
                Ok(inference_state
                    .resolve(last)
                    .expect("inferred type should resolve"))
            }
            Err(error) => {
                let fallback_range = module
                    .arena
                    .get(*module.expressions.first().expect("statement"))
                    .range;
                let diagnostic = Diagnostic::from_inference_error(
                    &error,
                    fallback_range,
                    lowering_context.interner(),
                );
                Err(diagnostic.message)
            }
        }
    }

    #[test]
    fn call_commits_the_first_candidate_that_accepts_the_arguments() {
        assert_eq!(
            infer_last_type("wrap(1L)\n").expect("integer candidate should match"),
            CoreType::Scalar(Atomic::Integer)
        );
        assert_eq!(
            infer_last_type("wrap(\"a\")\n").expect("character candidate should match"),
            CoreType::Scalar(Atomic::Character)
        );
    }

    #[test]
    fn call_no_candidate_accepts_reports_the_overload_set() {
        let message = infer_last_type("wrap(TRUE)\n").expect_err("no candidate accepts a logical");
        assert!(
            message.contains("no overload of `wrap` matches these arguments"),
            "message should name the overloaded callee: {message}"
        );
        assert!(
            message.contains("2 declared signatures"),
            "message should count the candidates: {message}"
        );
    }

    #[test]
    fn whole_number_literal_selects_by_courtesy_only_when_nothing_matches_exactly() {
        // `wrap` has no double candidate, so the literal courtesy admits `wrap(1)` through the
        // integer candidate on the second selection round.
        assert_eq!(
            infer_last_type("wrap(1)\n").expect("courtesy round should admit the literal"),
            CoreType::Scalar(Atomic::Integer)
        );
        // `sum` has a double candidate, so `sum(1, 2)` must select it — the courtesy must not let
        // the integer candidate win the strict round and misstate R's double result.
        assert_eq!(
            infer_last_type("sum(1, 2)\n").expect("sum should accept doubles"),
            CoreType::Scalar(Atomic::Double)
        );
    }
}
