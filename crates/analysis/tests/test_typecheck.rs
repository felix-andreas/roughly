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
        .unify(CoreType::Variable(variable), CoreType::Scalar(Atomic::Integer))
        .expect("variable should bind to integer");
    assert_eq!(
        inference_state.entry(variable),
        Some(&InferenceEntry::Bound(CoreType::Scalar(Atomic::Integer)))
    );

    inference_state.rollback_to(snapshot);

    // The variable allocated inside the snapshot is gone entirely (entry removed, id reclaimed).
    assert_eq!(inference_state.entry(variable), None);
    let reused = inference_state.fresh_variable();
    assert_eq!(reused, variable, "the reclaimed id should be handed out again");
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
        .unify(CoreType::Variable(permanent), CoreType::Scalar(Atomic::Integer))
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
