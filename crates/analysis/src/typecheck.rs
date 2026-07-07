use {
    crate::{
        document::DocumentId,
        hir::{
            Argument, AssignTarget, AssignmentScope, DefinitionKind, Expression, ExpressionId,
            ExpressionKind, HirArena, Module, contains_loop_exit, replacement_base,
        },
        interner::{Interner, Symbol},
        lower::LoweringContext,
        naming::{BindingId, NamesGlobal, NamesLocal, find_binding, find_exported_binding},
        types::{
            Annotation, Atomic, AttachedAnnotation, Constraint, CoreType, FunctionType,
            InferenceVariableId, QuantifiedVariable, RecordField, RestParameter, SurfaceType,
            TypeAnnotationKind, TypeScheme,
        },
    },
    std::collections::{BTreeMap, BTreeSet},
    tree_sitter::Range,
};

mod calls;
mod control;
mod environment;
mod operand;
mod unify;
use control::{GUARD_PREDICATES, GuardPredicate};
use operand::{
    ComparisonFamily, NumericOperand, NumericResultAtomic, OperandShape, alias_cycle_error,
    align_expected_parameter_types, classify_numeric_operand, combine_operand_atomic,
    comparison_operand_parts_list, core_type_for_shape, erase_variables,
    flexible_comparison_operand, integer_literal_position, is_whole_number_double_literal,
    literal_name_symbol, member_wise_numeric_results, nullable_type, promote_combine_atomic,
    shapes_for_operand, widen_error_container_to_union,
};

pub type Level = u32;

// Type-structure recursion (resolve, unification, compatibility, free-variable collection) follows
// the shape of annotation and inferred types. A pathologically nested type would otherwise recurse
// until the stack overflows; this bound turns that into a clean diagnostic instead of a crash. It is
// far deeper than any realistic type, yet low enough to fire well before the 2 MB worker/test-thread
// stack overflows (empirically around depth 200 for these passes).
pub(crate) const RECURSION_LIMIT: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceEntry {
    Unbound {
        level: Level,
        constraint: Constraint,
    },
    Redirect(InferenceVariableId),
    Bound(CoreType),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub type_scheme: TypeScheme,
    pub range: Range,
    // The slot is a defaultless parameter on a control-flow edge where `missing(name)` held, so a
    // read would fail at run time (R: "argument is missing, with no default"). Set only by the
    // missing()-guard refinement; any write to the slot clears it.
    pub unsupplied: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    Plus,
    Minus,
    Multiply,
    Divide,
    Power,
    Modulo,
    IntegerDivide,
    Colon,
    Compare,
    And,
    Or,
    Combine,
    List,
    Switch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandExpectation {
    Numeric,
    ScalarNumeric,
    Logical,
    Comparable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptKind {
    Position,
    FieldName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceError {
    UnknownInferenceVariable(InferenceVariableId),
    UnknownName {
        symbol: Symbol,
        range: Range,
        expression_id: ExpressionId,
    },
    AliasCycle {
        symbol: Symbol,
        range: Range,
        expression_id: Option<ExpressionId>,
    },
    ExpectedFunction {
        actual_type: Box<CoreType>,
        range: Range,
        expression_id: ExpressionId,
    },
    OccursCheckFailed {
        variable: InferenceVariableId,
        in_type: Box<CoreType>,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    TypeMismatch {
        expected: Box<CoreType>,
        actual: Box<CoreType>,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    UnresolvedAnnotationType {
        symbol: Symbol,
    },
    // A call to an overloaded stub name that no declared scheme accepts. `first_error` carries
    // the first candidate's failure for a concrete hint.
    NoMatchingOverload {
        symbol: Symbol,
        candidate_count: usize,
        range: Range,
        expression_id: ExpressionId,
        first_error: Option<Box<InferenceError>>,
    },
    // A `X[]` / `X[named]` annotation whose element is not an atomic type (or a type parameter):
    // vectors hold atomic elements only, and silently reading the annotation as `list[X]` hid the
    // mistake, so it is refused with a pointer at the list spelling.
    InvalidVectorElement {
        element: Box<CoreType>,
    },
    // An indexing form the checker does not model: multiple indexes (`m[i, j]`), an empty index
    // (`x[]`), or a named index argument. The subject was already inferred; this is about the
    // index shape, so the message must name indexing rather than a function call.
    UnsupportedIndexShape {
        index_count: usize,
        range: Range,
        expression_id: ExpressionId,
    },
    // An annotation names a parameter the annotated function does not define. R matches call
    // arguments against the definition's formal names, so such an annotation promises callers a
    // name the runtime would reject.
    AnnotationParameterNameMismatch {
        name: Symbol,
        range: Option<Range>,
    },
    ConstraintViolation {
        constraint: Constraint,
        actual: Box<CoreType>,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    InvalidOperand {
        expected: OperandExpectation,
        actual: Box<CoreType>,
        range: Range,
        expression_id: ExpressionId,
    },
    TupleLengthMismatch {
        expected: usize,
        actual: usize,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    MixedListElements {
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    RecordFieldMismatch {
        expected_fields: Vec<Symbol>,
        actual_fields: Vec<Symbol>,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    FunctionArityMismatch {
        expected: usize,
        actual: usize,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    NamedParameterMismatch {
        expected_parameters: Vec<Symbol>,
        actual_parameters: Vec<Symbol>,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    NotAList {
        actual: Box<CoreType>,
        range: Range,
        expression_id: ExpressionId,
    },
    NotIterable {
        actual: Box<CoreType>,
        range: Range,
        expression_id: ExpressionId,
    },
    MissingArgumentRead {
        symbol: Symbol,
        range: Range,
        expression_id: ExpressionId,
    },
    DollarOnAtomicVector {
        actual: Box<CoreType>,
        range: Range,
        expression_id: ExpressionId,
    },
    FieldDoesNotExist {
        field: Symbol,
        container: Box<CoreType>,
        range: Range,
        expression_id: ExpressionId,
    },
    PositionDoesNotExist {
        position: usize,
        container: Box<CoreType>,
        range: Range,
        expression_id: ExpressionId,
    },
    NonLiteralSubscript {
        container: Box<CoreType>,
        by: SubscriptKind,
        range: Range,
        expression_id: ExpressionId,
    },
    UnsupportedSubset {
        actual: Box<CoreType>,
        range: Range,
        expression_id: ExpressionId,
    },
    RecursionLimitExceeded,
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct InferenceState {
    next_variable_id: u32,
    current_level: Level,
    entries: BTreeMap<InferenceVariableId, InferenceEntry>,
    environment: BTreeMap<EnvironmentKey, Binding>,
    builtins: BTreeMap<Symbol, BuiltinKind>,
    // When enabled, every inferred expression's result type is recorded by id so tooling (hover,
    // inlay hints) can show checked types. Left off during interface rounds to avoid the cost.
    record_expression_types: bool,
    recorded_expression_types: BTreeMap<ExpressionId, CoreType>,
    // For each call whose callee resolved to a stub overload set, the index (into the declared set)
    // of the scheme the call committed, keyed by the callee expression. Only the selection pass
    // knows which candidate won, and signature help needs it to mark the active signature; recorded
    // on the commit path only, so failed probes leave nothing behind.
    selected_overloads: BTreeMap<ExpressionId, usize>,
    // One frame per function-literal body currently being inferred; each `return(x)` in the body
    // pushes its value's type into the innermost frame, and the function's return type is the
    // union of the frame with the body's trailing value. Transient: pushed and popped around one
    // body inference, so it is always empty at clone/compare points. Loop fixed-point re-passes
    // push duplicates, which the union normalization collapses.
    return_type_frames: Vec<Vec<CoreType>>,
    // Guard-predicate names (`is.null`, `is.character`, ...) resolved to symbols by the builtin
    // constructors, so `if` conditions can be inspected without string comparison. Empty for a
    // bare `InferenceState` (annotation harvesting never inspects conditions).
    guard_predicates: BTreeMap<Symbol, GuardPredicate>,
    // The symbol for `stop`, whose call unconditionally diverges (recognized by bare name, like
    // `local` and `return` at lowering). `None` for a bare state.
    stop_symbol: Option<Symbol>,
    missing_symbol: Option<Symbol>,
    // The current function frame's defaultless parameter slots — the only slots a `missing(name)`
    // guard may narrow. Saved and replaced (not extended) around each body: R's `missing()`
    // applies only to the immediate function's own formals.
    missing_narrowable: BTreeSet<BindingId>,
    // Sites that introduce a genuine `Unknown` into the type lattice, collected while recording is
    // enabled (the authoritative round-2 check). Strict mode reports these origins; ordinary
    // propagation of `Unknown` is never recorded here, so a single root cause yields one diagnostic.
    // Drained and filtered to still-`Unknown` recorded types by `check_module_with_naming`.
    strict_origins: Vec<StrictUnknownOrigin>,
    // Rigid (skolem) variables introduced by a `<T>` annotation binder while checking a function
    // body. They model a universally quantified parameter: the body must work for *every* T, so a
    // rigid variable refuses to be bound to a concrete type or constrained. After the check they are
    // ordinary free variables again and generalize back into the `<T>` scheme. The map keeps each
    // one's declared name so diagnostics show `T` rather than an internal `type1`.
    rigid_variables: BTreeMap<InferenceVariableId, Symbol>,
    // Current depth of type-structure recursion, bounded by `RECURSION_LIMIT`. Transient: it returns
    // to zero whenever a top-level traversal finishes, so it carries no logical state (it is always
    // zero at the points where an `InferenceState` is cloned or compared). It is deliberately NOT
    // captured by `Snapshot` and NOT touched by snapshot/rollback/commit: a snapshot reverses
    // union-find writes only, never the in-flight recursion counter.
    recursion_depth: usize,
    // ena-style undo log for speculative unification. While a snapshot is active (`snapshot_depth >
    // 0`) every union-find write records the prior entry here so it can be reversed on rollback; on
    // the normal committed path (`snapshot_depth == 0`) nothing is recorded, so the log stays empty
    // and cannot grow unbounded. Empty (and `snapshot_depth == 0`) at every clone/compare point,
    // because recording only happens inside a probe that always commits or rolls back before
    // returning — so the derived `Clone`/`PartialEq` stay correct.
    undo_log: Vec<UndoStep>,
    snapshot_depth: usize,
    // Undo log for the *environment map*, the same shape as `undo_log` for the union-find: while an
    // environment snapshot is active every `set_environment_entry` records the key's prior value so
    // a control-flow region (a branch, a loop pass, a function body) can be reverted without
    // cloning the whole environment (which holds every stdlib stub scheme). Empty (and depth 0) at
    // every clone/compare point: regions always roll back before returning.
    environment_log: Vec<(EnvironmentKey, Option<Binding>)>,
    environment_snapshot_depth: usize,
    // Super-assignment (`<<-`) writes recorded while checking a function body. The body's
    // environment region rolls back when the signature is done, but a super-assignment mutates an
    // *enclosing* frame's slot, so the recorded writes re-join into the environment at the
    // function's definition site (and again at each further enclosing definition site — the join
    // is idempotent). Cleared per top-level statement.
    pending_enclosing_writes: Vec<(EnvironmentKey, CoreType, Range)>,
    // Ordered overload sets for stub names (declaration order). A call whose callee is one of
    // these names tries each scheme with a probe and commits the first that accepts the
    // arguments; every non-call use of the name sees the first scheme (the environment binding).
    overload_sets: BTreeMap<Symbol, Vec<TypeScheme>>,
    // Non-zero while matching arguments against a probed overload candidate. The whole-number
    // literal courtesy (`1` accepted where `integer` is expected) is disabled during probes: the
    // literal is genuinely a double at runtime, so letting it match an integer candidate would
    // select a signature whose return type misstates what R computes.
    overload_probe_depth: usize,
    // Per captured slot flagged for the discovery re-pass: the running join of every write's type
    // (variables erased to `Unknown` so entries survive unification rollbacks). Captured reads of
    // such slots resolve here instead of the definition-point environment entry, making them sound
    // for closure calls that happen after later writes.
    captured_write_joins: BTreeMap<BindingId, CoreType>,
    // Set when the current walk wrote a slot in `NamesLocal::capture_repass_slots`; the innermost
    // enclosing function body re-runs once so its captured reads see the completed write join.
    wrote_repass_slot: bool,
    // Per loop region: the memoized (guard, exit effects) of the last converged run, plus the
    // stack of read/write logs recording what active loop regions depend on. See
    // `infer_loop_to_fixed_point`.
    loop_memos: BTreeMap<ExpressionId, LoopMemo>,
    loop_access_logs: Vec<LoopAccessLog>,
}

// The memoized outcome of one loop region's fixed point. `guard` maps every environment key the
// region's passes read or wrote to its value at region entry; when all guard values match the
// current environment, re-running the region would unfold identically, so `exit_effects` (the
// entries the region left behind) applies directly. Regions whose later passes touched keys their
// first pass did not are not memoized (the guard would under-approximate the read set).
#[derive(Debug, Clone, PartialEq, Eq)]
struct LoopMemo {
    guard: BTreeMap<EnvironmentKey, Option<Binding>>,
    exit_effects: BTreeMap<EnvironmentKey, Option<Binding>>,
    // The strict origins the memoized run recorded (loop widenings and origins inside the body),
    // replayed through the deduplicating recorder so a discovery pass's truncation cannot lose
    // them.
    origins: Vec<StrictUnknownOrigin>,
}

// Records the environment keys one active loop region has touched, with each key's value at first
// touch. `complete` turns false when a pass beyond the first touches a new key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct LoopAccessLog {
    accesses: BTreeMap<EnvironmentKey, Option<Binding>>,
    first_pass: bool,
    complete: bool,
}

// A single reversible union-find write: `previous` is the entry that existed before the write
// (`None` if the variable was freshly inserted, so rollback removes the key).
#[derive(Debug, Clone, PartialEq, Eq)]
enum UndoStep {
    Entry {
        variable: InferenceVariableId,
        previous: Option<InferenceEntry>,
    },
    // A rigid (skolem) marker written by `fresh_rigid_variable`. The id is always fresh, so
    // `previous` is `None` in practice, but recording it keeps rollback symmetric with `Entry` and
    // robust if the marker is ever re-set on an existing id.
    Rigid {
        variable: InferenceVariableId,
        previous: Option<Symbol>,
    },
}

// A cheap marker into the undo log. Rolling back truncates the log to `log_len` and restores
// `next_variable_id`, reclaiming any variables allocated since the snapshot. It captures no entries
// and never references `recursion_depth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    log_len: usize,
    next_variable_id: u32,
}

// A marker into the environment log (see `environment_log`). Rolling back restores every
// environment entry written since the snapshot and hands the caller the region's final values, so
// control-flow joins can merge them with the pre-state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EnvironmentSnapshot {
    log_length: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EnvironmentKey {
    Local(BindingId),
    Global(Symbol),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleCheck {
    pub expression_types: Vec<CoreType>,
    pub expression_types_by_id: BTreeMap<ExpressionId, CoreType>,
    // The constraint of every still-unbound inference variable occurring in the recorded types.
    // Display-time generalization (hover, inlay hints, signature help) quantifies those variables
    // and needs the bound to render `<T: numeric>` rather than a bare `<T>`; the inference state
    // that knows it is gone by the time the IDE layer reads the stored types.
    pub variable_constraints: BTreeMap<InferenceVariableId, Constraint>,
    // For each call whose callee resolved to a stub overload set, the declared-set index of the
    // committed scheme, keyed by the callee expression. Signature help lists the whole set and
    // marks this one active.
    pub selected_overloads: BTreeMap<ExpressionId, usize>,
    pub errors: Vec<InferenceError>,
    // Origins of genuine `Unknown` types, each already confirmed to resolve to `Unknown` in the
    // final substitution. Strict mode turns each into one diagnostic; non-strict checks ignore them.
    pub strict_origins: Vec<StrictUnknownOrigin>,
}

// A site that first introduces a non-error `Unknown` into the type lattice. Strict mode flags these
// origins; it never flags expressions that merely propagate `Unknown` from a child or referenced
// binding (see the strict-mode section of the typing reference).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrictUnknownOrigin {
    pub expression_id: ExpressionId,
    pub range: Range,
    pub kind: StrictOriginKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrictOriginKind {
    // A syntactically valid construct the checker does not yet model.
    UnsupportedConstruct,
    // A name reference whose resolved binding has no known type (a base-environment or library
    // binding that is itself `Unknown`). Composes with library typing: once the binding is stubbed,
    // it resolves to a real type and is no longer an origin.
    UndeterminedReference(Symbol),
    // A variable whose type kept growing across loop iterations and was widened to `Unknown` at
    // the fixed-point pass cap (the termination safety net). The `Unknown` lives in a variable
    // slot rather than an expression, so strict mode keeps these origins regardless of the loop
    // expression's own recorded type.
    LoopWidened(Symbol),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedValue {
    pub symbol: Symbol,
    pub type_scheme: TypeScheme,
    pub range: Range,
}

struct ResolutionContext<'a> {
    document_id: DocumentId,
    module: &'a Module,
    top_level_expression_ids: &'a [ExpressionId],
    local_naming: &'a NamesLocal,
    package_naming: &'a NamesGlobal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeDefinition {
    kind: DefinitionKind,
    type_parameters: Vec<Symbol>,
    // `None` for an opaque nominal (a stub `@type`): there is nothing to expand, so structural
    // projection, variance computation, and representation checks all treat the type as sealed —
    // it is compatible only with itself.
    representation: Option<SurfaceType>,
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct TypeDefinitionEnvironment {
    definitions: BTreeMap<Symbol, TypeDefinition>,
}

impl TypeDefinitionEnvironment {
    pub fn from_module(module: &Module) -> Self {
        Self::from_modules([module])
    }

    pub fn from_modules<'a>(modules: impl IntoIterator<Item = &'a Module>) -> Self {
        let mut definitions = BTreeMap::new();

        for module in modules {
            for definition in &module.definitions {
                definitions.insert(
                    definition.definition.name,
                    TypeDefinition {
                        kind: definition.definition.kind,
                        type_parameters: definition.definition.type_parameters.clone(),
                        representation: Some(definition.definition.surface_type.clone()),
                    },
                );
            }
        }

        Self { definitions }
    }

    // Seeds an opaque nominal type (kind `Type`, no parameters, `Any` representation) under the
    // module-declared definitions: standard-library stub types (`data.frame`, `connection`, ...)
    // enter here, and a module's own `@type`/`@alias` of the same name wins because seeding never
    // overwrites an existing definition. The `Any` representation is honest for an opaque type —
    // its structure is not inspectable, so `@new` against it checks nothing and compatibility
    // works purely by name.
    pub fn seed_opaque_type(&mut self, symbol: Symbol) {
        self.definitions.entry(symbol).or_insert(TypeDefinition {
            kind: DefinitionKind::Type,
            type_parameters: Vec::new(),
            representation: None,
        });
    }

    fn get(&self, symbol: Symbol) -> Option<&TypeDefinition> {
        self.definitions.get(&symbol)
    }
}

// Variance of a nominal type parameter, derived from where the parameter occurs in the
// representation type. It controls the direction each type argument is checked in the
// Nominal-vs-Nominal compatibility arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variance {
    // The parameter does not occur in the representation; it constrains nothing, so any argument is
    // accepted unconditionally. Acts as the identity for `join`.
    Bivariant,
    Covariant,
    Contravariant,
    Invariant,
}

impl Variance {
    // Combines two occurrences of the same parameter: equal stays equal, `Bivariant` is the
    // identity, and any disagreement (co with contra, or anything with invariant) becomes invariant.
    fn join(self, other: Variance) -> Variance {
        match (self, other) {
            (Variance::Bivariant, value) | (value, Variance::Bivariant) => value,
            (left, right) if left == right => left,
            _ => Variance::Invariant,
        }
    }

    // Flips polarity when descending into a contravariant position (a function parameter).
    fn flip(self) -> Variance {
        match self {
            Variance::Covariant => Variance::Contravariant,
            Variance::Contravariant => Variance::Covariant,
            other => other,
        }
    }
}

// Computes the variance of each of a definition's type parameters (aligned with
// `definition.type_parameters`) from its occurrences in the representation type. Function-parameter
// positions flip polarity; function-return, container/structural, and direct positions preserve it;
// nested nominal arguments are treated conservatively as `Invariant` (a precise nested-nominal
// fixpoint is a deferred refinement); multiple occurrences join. A parameter that never occurs
// stays `Bivariant`.
fn parameter_variances(definition: &TypeDefinition) -> Vec<Variance> {
    let mut variances = BTreeMap::new();
    if let Some(representation) = &definition.representation {
        accumulate_parameter_variances(
            representation,
            Variance::Covariant,
            &definition.type_parameters,
            &mut variances,
        );
    }
    definition
        .type_parameters
        .iter()
        .map(|parameter| {
            variances
                .get(parameter)
                .copied()
                .unwrap_or(Variance::Bivariant)
        })
        .collect()
}

fn accumulate_parameter_variances(
    surface_type: &SurfaceType,
    polarity: Variance,
    parameters: &[Symbol],
    variances: &mut BTreeMap<Symbol, Variance>,
) {
    match surface_type {
        SurfaceType::Named(name, arguments) => {
            if parameters.contains(name) {
                let entry = variances.entry(*name).or_insert(Variance::Bivariant);
                *entry = entry.join(polarity);
            }
            // A nested generic application's arguments are treated conservatively as invariant: we do
            // not yet compose the inner nominal's own per-parameter variance, so any parameter that
            // occurs inside such an argument joins to `Invariant` (sound — it neither over-accepts a
            // widening nor a narrowing). Precise composition is a deferred refinement.
            for argument in arguments {
                accumulate_parameter_variances(
                    argument,
                    Variance::Invariant,
                    parameters,
                    variances,
                );
            }
        }
        SurfaceType::Function(function_type) => {
            for parameter in &function_type.parameters {
                accumulate_parameter_variances(parameter, polarity.flip(), parameters, variances);
            }
            for named_parameter in &function_type.named_parameters {
                accumulate_parameter_variances(
                    &named_parameter.value,
                    polarity.flip(),
                    parameters,
                    variances,
                );
            }
            // A rest parameter is a parameter position, so its element is contravariant like the fixed
            // parameters.
            if let Some(variadic) = &function_type.variadic {
                accumulate_parameter_variances(
                    &variadic.element,
                    polarity.flip(),
                    parameters,
                    variances,
                );
            }
            accumulate_parameter_variances(
                &function_type.return_type,
                polarity,
                parameters,
                variances,
            );
        }
        SurfaceType::Union(members) => {
            for member in members {
                accumulate_parameter_variances(member, polarity, parameters, variances);
            }
        }
        SurfaceType::Vector(inner)
        | SurfaceType::NamedVector(inner)
        | SurfaceType::List(inner)
        | SurfaceType::NamedList(inner)
        | SurfaceType::Binders(_, inner) => {
            accumulate_parameter_variances(inner, polarity, parameters, variances);
        }
        SurfaceType::Record(fields) => {
            for field in fields {
                accumulate_parameter_variances(&field.value, polarity, parameters, variances);
            }
        }
        SurfaceType::Tuple(items) => {
            for item in items {
                accumulate_parameter_variances(item, polarity, parameters, variances);
            }
        }
        SurfaceType::Any | SurfaceType::Unknown | SurfaceType::Null | SurfaceType::Scalar(_) => {}
    }
}

pub fn inference_state_with_builtins(lowering_context: &mut LoweringContext) -> InferenceState {
    inference_state_with_builtins_in_interner(lowering_context.interner_mut())
}

pub fn inference_state_with_builtins_in_interner(interner: &mut Interner) -> InferenceState {
    let mut inference_state = InferenceState::new();

    for (name, builtin_kind) in BUILTINS {
        let symbol = interner.intern(name);
        inference_state.bind_builtin(symbol, *builtin_kind);
    }
    for (name, predicate) in GUARD_PREDICATES {
        let symbol = interner.intern(name);
        inference_state.guard_predicates.insert(symbol, *predicate);
    }
    inference_state.stop_symbol = Some(interner.intern("stop"));
    inference_state.missing_symbol = Some(interner.intern("missing"));

    inference_state
}

pub const BUILTINS: &[(&str, BuiltinKind)] = &[
    ("+", BuiltinKind::Plus),
    ("-", BuiltinKind::Minus),
    ("*", BuiltinKind::Multiply),
    ("/", BuiltinKind::Divide),
    ("**", BuiltinKind::Power),
    ("^", BuiltinKind::Power),
    ("%%", BuiltinKind::Modulo),
    ("%/%", BuiltinKind::IntegerDivide),
    (":", BuiltinKind::Colon),
    ("<", BuiltinKind::Compare),
    ("<=", BuiltinKind::Compare),
    (">", BuiltinKind::Compare),
    (">=", BuiltinKind::Compare),
    ("==", BuiltinKind::Compare),
    ("!=", BuiltinKind::Compare),
    ("&&", BuiltinKind::And),
    ("||", BuiltinKind::Or),
    ("c", BuiltinKind::Combine),
    ("list", BuiltinKind::List),
    ("switch", BuiltinKind::Switch),
];

impl InferenceState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enable_expression_type_recording(&mut self) {
        self.record_expression_types = true;
    }

    // Records a strict-mode `Unknown` origin, but only while expression-type recording is on (the
    // authoritative round-2 check). The interface rounds discard their `ModuleCheck`, so collecting
    // origins there would be wasted work and would leave the buffer non-empty in cloned states.
    // Idempotent per expression: loop bodies are re-inferred to a control-flow fixed point, so the
    // same origin site can be visited more than once but must yield one diagnostic.
    fn record_strict_origin(
        &mut self,
        expression_id: ExpressionId,
        range: Range,
        kind: StrictOriginKind,
    ) {
        // Deduplication includes the kind: a loop that widens several variables records one
        // `LoopWidened` origin per variable on the same loop expression.
        if self.record_expression_types
            && !self
                .strict_origins
                .iter()
                .any(|origin| origin.expression_id == expression_id && origin.kind == kind)
        {
            self.strict_origins.push(StrictUnknownOrigin {
                expression_id,
                range,
                kind,
            });
        }
    }

    // Resolves every recorded expression type against the final substitution and clears the
    // buffer. Variables left unbound (for example a generalized numeric parameter) resolve to
    // themselves; resolution failures degrade to `Unknown` so a display feature never aborts a check.
    fn take_recorded_expression_types(&mut self) -> BTreeMap<ExpressionId, CoreType> {
        let recorded = std::mem::take(&mut self.recorded_expression_types);
        recorded
            .into_iter()
            .map(|(expression_id, core_type)| {
                (
                    expression_id,
                    self.resolve(core_type).unwrap_or(CoreType::Unknown),
                )
            })
            .collect()
    }

    pub fn infer_module(
        &mut self,
        module: &Module,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<Vec<CoreType>, InferenceError> {
        self.infer_module_with_context(module, None, type_definitions)
    }

    // Checking recovers per top-level expression: one inference error poisons only its own
    // expression, which continues as `Unknown`, so every error in a document is reported.
    pub fn check_module_with_naming(
        &mut self,
        document_id: DocumentId,
        module: &Module,
        local_naming: &NamesLocal,
        package_naming: &NamesGlobal,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> ModuleCheck {
        let resolution_context = ResolutionContext {
            document_id,
            module,
            top_level_expression_ids: &module.expressions,
            local_naming,
            package_naming,
        };
        // Loop memos and captured-write joins are keyed by per-module expression/binding ids;
        // clear them so a state reused across documents cannot alias.
        self.loop_memos.clear();
        self.captured_write_joins.clear();

        // When a top-level slot is captured by a closure and written after that capture, the
        // whole document runs a discovery pass first (the per-function-body re-pass cannot see
        // top-level writes made after the closure): the first pass completes the captured-write
        // joins and then rolls back entirely, exactly like the function-body discovery.
        let passes = if local_naming.top_level_capture_repass {
            2
        } else {
            1
        };
        let mut expression_types = Vec::new();
        let mut errors = Vec::new();
        for pass in 0..passes {
            let discovery = pass + 1 < passes;
            expression_types = Vec::with_capacity(module.expressions.len());
            errors = Vec::new();
            let environment_snapshot = discovery.then(|| self.environment_snapshot());
            let unification_snapshot = discovery.then(|| self.snapshot());
            for expression_id in &module.expressions {
                // Super-assign writes propagate to enclosing definition sites within one
                // statement; they must not leak across statements.
                self.pending_enclosing_writes.clear();
                let expression = module.arena.get(*expression_id);
                match self.infer_expression_with_context(
                    expression,
                    &module.arena,
                    Some(&resolution_context),
                    type_definitions,
                ) {
                    Ok(expression_type) => expression_types.push(expression_type),
                    Err(error) => {
                        errors.push(error);
                        expression_types.push(CoreType::Unknown);
                        if matches!(expression.kind, ExpressionKind::Assign { .. }) {
                            // Later references reach a failed top-level binding through both the
                            // local and the package-global lookup path, so recovery binds both.
                            let recovery_scheme = TypeScheme::monomorphic(CoreType::Unknown);
                            if let Some(binding_id) = local_naming
                                .expression_resolutions
                                .get(expression_id)
                                .copied()
                            {
                                self.bind_local_scheme(
                                    binding_id,
                                    recovery_scheme.clone(),
                                    expression.range,
                                );
                                if local_naming.capture_repass_slots.contains(&binding_id) {
                                    self.wrote_repass_slot = true;
                                    self.captured_write_joins
                                        .insert(binding_id, CoreType::Unknown);
                                }
                            }
                            if let Some(target) = expression.kind.assignment_variable()
                                && package_naming.global_bindings.contains_key(&target)
                            {
                                self.bind_global_scheme(target, recovery_scheme, expression.range);
                            }
                        }
                    }
                }
            }
            if discovery {
                if let Some(snapshot) = environment_snapshot {
                    self.environment_rollback(snapshot);
                }
                if let Some(snapshot) = unification_snapshot {
                    self.rollback_to(snapshot);
                }
                self.strict_origins.clear();
                self.recorded_expression_types.clear();
                self.selected_overloads.clear();
                self.loop_memos.clear();
            }
        }
        self.pending_enclosing_writes.clear();
        self.wrote_repass_slot = false;

        let expression_types_by_id = self.take_recorded_expression_types();
        // Keep only origins whose expression still resolves to `Unknown` in the final substitution.
        // This drops any origin whose `Unknown` was later overridden (for example a `@trust Any`
        // annotation makes the expression `Any`, which strict mode tolerates) and any origin under a
        // top-level statement that failed to type-check (its error is recorded separately and the
        // failed expression is never recorded here, so no double-report). Loop widenings are kept
        // unconditionally: their `Unknown` lives in a variable slot, not in the loop expression's
        // own recorded type.
        let strict_origins = std::mem::take(&mut self.strict_origins)
            .into_iter()
            .filter(|origin| {
                matches!(origin.kind, StrictOriginKind::LoopWidened(_))
                    || expression_types_by_id.get(&origin.expression_id) == Some(&CoreType::Unknown)
            })
            .collect();
        let mut variable_constraints = BTreeMap::new();
        for core_type in expression_types_by_id
            .values()
            .chain(expression_types.iter())
            .cloned()
            .collect::<Vec<_>>()
        {
            if let Ok(free_variables) = self.free_type_variables(&core_type) {
                for variable in free_variables {
                    if let Some(InferenceEntry::Unbound { constraint, .. }) =
                        self.entries.get(&variable)
                        && *constraint != Constraint::Unconstrained
                    {
                        variable_constraints.insert(variable, *constraint);
                    }
                }
            }
        }
        ModuleCheck {
            expression_types,
            expression_types_by_id,
            variable_constraints,
            selected_overloads: std::mem::take(&mut self.selected_overloads),
            errors,
            strict_origins,
        }
    }

    pub fn exported_value_schemes(
        &self,
        module: &Module,
        local_naming: &NamesLocal,
    ) -> Vec<ExportedValue> {
        let mut symbols_in_order = Vec::new();
        for expression_id in &module.expressions {
            if let Some(target) = module
                .arena
                .get(*expression_id)
                .kind
                .simple_assignment_target()
                && !symbols_in_order.contains(&target)
            {
                symbols_in_order.push(target);
            }
        }

        symbols_in_order
            .into_iter()
            .filter_map(|symbol| {
                let binding_id = find_exported_binding(module, local_naming, symbol)?;
                let binding = self
                    .lookup_local_name(binding_id)
                    .or_else(|| self.lookup_global_name(symbol))?;
                Some(ExportedValue {
                    symbol,
                    type_scheme: binding.type_scheme.clone(),
                    range: binding.range,
                })
            })
            .collect()
    }

    // Harvests a stub annotation's surface type into a `TypeScheme` without inferring any body, used
    // by `StubLibrary::load` to turn declaration-only base stubs into schemes through the ordinary
    // lowering + generalization path.
    pub fn harvest_annotation_scheme(
        &mut self,
        surface_type: &SurfaceType,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<TypeScheme, InferenceError> {
        let core_type = self.lower_annotation_surface_type(surface_type, type_definitions, None)?;
        self.generalize_annotation(core_type)
    }

    // Generalizes a type lowered directly from an annotation (a stub declaration or `@trust` type),
    // where there is no function body to check against. A `<T>` binder lowers to a rigid variable; with
    // no body-inference step to turn it back into an ordinary free variable, ordinary `generalize`
    // (which quantifies only level-scoped unbound variables) would leave it un-quantified and the
    // resulting scheme monomorphic-but-open. Here every rigid variable in the lowered type is a
    // universally quantified parameter, so quantify them alongside the normally generalizable ones.
    fn generalize_annotation(&mut self, core_type: CoreType) -> Result<TypeScheme, InferenceError> {
        let resolved_type = self.resolve(core_type)?;
        let type_variables = self.free_type_variables_in_core_type(&resolved_type)?;

        let mut quantified_variables = Vec::new();
        for variable in type_variables {
            let Some(entry) = self.entries.get(&variable) else {
                return Err(InferenceError::UnknownInferenceVariable(variable));
            };
            if let InferenceEntry::Unbound { level, constraint } = entry {
                let constraint = *constraint;
                if *level > self.current_level || self.rigid_variables.contains_key(&variable) {
                    quantified_variables.push(QuantifiedVariable::new(variable, constraint));
                }
            }
        }

        Ok(TypeScheme {
            quantified_variables,
            body: resolved_type,
        })
    }

    fn infer_module_with_context(
        &mut self,
        module: &Module,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<Vec<CoreType>, InferenceError> {
        let mut inferred_types = Vec::with_capacity(module.expressions.len());

        for expression_id in &module.expressions {
            let expression = module.arena.get(*expression_id);
            inferred_types.push(self.infer_expression_with_context(
                expression,
                &module.arena,
                resolution_context,
                type_definitions,
            )?);
        }

        Ok(inferred_types)
    }

    pub fn infer_expression(
        &mut self,
        expression: &Expression,
        arena: &HirArena,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        self.infer_expression_with_context(expression, arena, None, type_definitions)
    }

    fn infer_expression_with_context(
        &mut self,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let inferred_type =
            self.infer_expression_kind(expression, arena, resolution_context, type_definitions)?;
        // Apply an expression-level annotation such as `#: @new User` on a bare expression (for
        // example a block's final expression). Annotations that also bind a name are applied by the
        // assignment path instead, so they are skipped here to avoid double application.
        let annotated_type = match &expression.annotation {
            Some(annotation) if !annotation.applies_to_binding() => {
                self.apply_annotation(annotation, inferred_type, expression, type_definitions)?
            }
            _ => inferred_type,
        };
        if self.record_expression_types {
            self.recorded_expression_types
                .insert(expression.id, annotated_type.clone());
        }
        Ok(annotated_type)
    }

    fn infer_expression_kind(
        &mut self,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        match &expression.kind {
            ExpressionKind::Null => Ok(CoreType::Null),
            ExpressionKind::Logical(_) => Ok(CoreType::Scalar(Atomic::Logical)),
            ExpressionKind::Integer(_) => Ok(CoreType::Scalar(Atomic::Integer)),
            ExpressionKind::Double(_) => Ok(CoreType::Scalar(Atomic::Double)),
            ExpressionKind::Character(_) => Ok(CoreType::Scalar(Atomic::Character)),
            ExpressionKind::AtomicConstant(atomic) => Ok(CoreType::Scalar(*atomic)),
            ExpressionKind::StringLiteralName(_) => Ok(CoreType::Scalar(Atomic::Character)),
            ExpressionKind::Symbol(symbol) => {
                if let Some(resolution_context) = resolution_context {
                    if let Some(binding_id) = resolution_context
                        .local_naming
                        .expression_resolutions
                        .get(&expression.id)
                    {
                        self.log_loop_access(EnvironmentKey::Local(*binding_id));
                        // A defaultless parameter on a `missing(name)`-true edge: R would fail
                        // this read at run time, so it is an error, not a type.
                        if let Some(binding) = self.lookup_local_name(*binding_id)
                            && binding.unsupplied
                        {
                            return Err(InferenceError::MissingArgumentRead {
                                symbol: *symbol,
                                range: expression.range,
                                expression_id: expression.id,
                            });
                        }
                        // A read captured by a closure must stay sound for calls made after later
                        // writes to the slot, so it resolves to the accumulated join of all the
                        // frame's writes rather than the definition-point entry (only slots that
                        // actually have post-capture writes carry a join; see
                        // `capture_repass_slots`).
                        if resolution_context
                            .local_naming
                            .captured_reads
                            .contains(&expression.id)
                            && resolution_context
                                .local_naming
                                .capture_repass_slots
                                .contains(binding_id)
                            && let Some(join) = self.captured_write_joins.get(binding_id)
                        {
                            return Ok(join.clone());
                        }
                        // A local reference can resolve to a binding that inference has not bound
                        // yet: a forward or recursive reference, or a binding introduced only in a
                        // conditionally executed branch (if/for/while/repeat). Such a binding has no
                        // known type, so it is `Unknown`. We must not panic here — IDE requests
                        // (hover, inlay hints) reach this path and crashing the server is not an
                        // option.
                        return match self.lookup_local_name(*binding_id) {
                            Some(binding) => {
                                let type_scheme = binding.type_scheme.clone();
                                self.instantiate_type_scheme(&type_scheme)
                            }
                            None => Ok(CoreType::Unknown),
                        };
                    }

                    if resolution_context
                        .local_naming
                        .non_locals
                        .contains_key(&expression.id)
                    {
                        self.log_loop_access(EnvironmentKey::Global(*symbol));
                        if resolution_context
                            .package_naming
                            .global_bindings
                            .contains_key(symbol)
                        {
                            let type_scheme = self
                                .lookup_global_name(*symbol)
                                .unwrap_or_else(|| {
                                    panic!(
                                        "package global symbol {:?} should be prebound for typecheck",
                                        symbol
                                    )
                                })
                                .type_scheme
                                .clone();
                            return self.instantiate_type_scheme(&type_scheme);
                        }

                        // Not a package global, but it may be a seeded stdlib stub bound into the
                        // base template environment (e.g. `length`, `T`, `pi`). Such a base scheme
                        // is not a package global, so resolve it directly from the environment;
                        // only a genuinely unresolved non-local (naming already reported "could not
                        // resolve") falls through to `Unknown`.
                        if let Some(binding) = self.lookup_global_name(*symbol) {
                            let type_scheme = binding.type_scheme.clone();
                            return self.instantiate_type_scheme(&type_scheme);
                        }

                        return Ok(CoreType::Unknown);
                    }
                }

                self.log_loop_access(EnvironmentKey::Global(*symbol));
                let Some(binding) = self.lookup_global_name(*symbol).cloned() else {
                    return Err(InferenceError::UnknownName {
                        symbol: *symbol,
                        range: expression.range,
                        expression_id: expression.id,
                    });
                };
                let resolved_type = self.instantiate_type_scheme(&binding.type_scheme)?;
                // A base-environment or library binding that has no known type is a strict origin:
                // the reference is where that `Unknown` enters the lattice. Package-global and local
                // bindings reach `Unknown` through the paths above instead, so their origin is the
                // defining site, not this reference.
                if resolved_type == CoreType::Unknown {
                    self.record_strict_origin(
                        expression.id,
                        expression.range,
                        StrictOriginKind::UndeterminedReference(*symbol),
                    );
                }
                Ok(resolved_type)
            }
            ExpressionKind::Block {
                expressions,
                has_trailing_semicolon,
            } => self.infer_block(
                expressions,
                *has_trailing_semicolon,
                arena,
                resolution_context,
                type_definitions,
            ),
            ExpressionKind::Assign {
                target,
                scope,
                value,
            } => match (target, scope) {
                (AssignTarget::Variable { symbol, .. }, AssignmentScope::Local) => self
                    .infer_local_variable_assign(
                        *symbol,
                        *value,
                        expression,
                        arena,
                        resolution_context,
                        type_definitions,
                    ),
                (AssignTarget::Variable { symbol, .. }, AssignmentScope::Enclosing) => self
                    .infer_super_assign(
                        *symbol,
                        *value,
                        expression,
                        arena,
                        resolution_context,
                        type_definitions,
                    ),
                (AssignTarget::Replacement { lhs }, _) => self.infer_replacement_assign(
                    *lhs,
                    *value,
                    *scope,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                ),
            },
            ExpressionKind::Function {
                parameters,
                variadic,
                body,
            } => self.infer_function_expression(
                expression.id,
                parameters,
                *variadic,
                *body,
                None,
                expression,
                arena,
                resolution_context,
                type_definitions,
            ),
            // `local(expr)` evaluates its body in a fresh scope and returns its value, so the expression's
            // type is the body's type. Scope isolation is a naming concern (inner assignments bind locally
            // and do not leak); inference just threads the body's value type through.
            ExpressionKind::Local { body } => self.infer_expression_with_context(
                arena.get(*body),
                arena,
                resolution_context,
                type_definitions,
            ),
            ExpressionKind::If {
                condition,
                consequence,
                alternative,
            } => self.infer_if_expression(
                arena.get(*condition),
                arena.get(*consequence),
                alternative.as_ref().map(|id| arena.get(*id)),
                expression,
                arena,
                resolution_context,
                type_definitions,
            ),
            ExpressionKind::For {
                variable,
                sequence,
                body,
            } => self.infer_for_expression(
                expression.id,
                *variable,
                arena.get(*sequence),
                arena.get(*body),
                expression,
                arena,
                resolution_context,
                type_definitions,
            ),
            ExpressionKind::While { condition, body } => self.infer_while_expression(
                arena.get(*condition),
                arena.get(*body),
                expression,
                arena,
                resolution_context,
                type_definitions,
            ),
            ExpressionKind::Repeat { body } => self.infer_repeat_expression(
                arena.get(*body),
                expression,
                arena,
                resolution_context,
                type_definitions,
            ),
            ExpressionKind::UnaryMinus { value } => self.infer_unary_minus(
                arena.get(*value),
                arena,
                resolution_context,
                type_definitions,
            ),
            ExpressionKind::UnaryNot { value } => self.infer_unary_not(
                arena.get(*value),
                arena,
                resolution_context,
                type_definitions,
            ),
            ExpressionKind::Call { callee, arguments } => {
                let callee_expr = arena.get(*callee);
                if let ExpressionKind::Symbol(symbol) = &callee_expr.kind
                    && let Some(inferred_type) = self.infer_builtin_call(
                        *symbol,
                        arguments,
                        expression,
                        arena,
                        resolution_context,
                        type_definitions,
                    )?
                {
                    return Ok(inferred_type);
                }

                self.infer_function_call_expression(
                    callee_expr,
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
            }
            ExpressionKind::Subset { value, arguments } => self.infer_subset_expression(
                arena.get(*value),
                arguments,
                expression,
                arena,
                resolution_context,
                type_definitions,
            ),
            ExpressionKind::Subset2 { value, arguments } => self.infer_subset2_expression(
                arena.get(*value),
                arguments,
                expression,
                arena,
                resolution_context,
                type_definitions,
            ),
            ExpressionKind::Dollar { value, name } => self.infer_dollar_expression(
                arena.get(*value),
                *name,
                expression,
                arena,
                resolution_context,
                type_definitions,
            ),
            // `return(x)` exits the enclosing function: the value's type joins that function's
            // return type (collected on the frame the function-literal inference pushed). Like
            // `break`/`next`, the expression yields no observable value where it stands, so it
            // types as `NULL` locally and is not a strict origin. A top-level `return` (an R
            // runtime error) has no frame to feed; its value is still checked.
            ExpressionKind::Return { value } => {
                let returned = match value {
                    Some(value) => {
                        let inferred = self.infer_expression_with_context(
                            arena.get(*value),
                            arena,
                            resolution_context,
                            type_definitions,
                        )?;
                        self.resolve(inferred)?
                    }
                    None => CoreType::Null,
                };
                if let Some(frame) = self.return_type_frames.last_mut() {
                    frame.push(returned);
                }
                Ok(CoreType::Null)
            }
            // `break` and `next` transfer control and never produce an observable value; like the
            // loops they belong to, they type as `NULL`. They are fully understood constructs, so
            // they are not strict origins.
            ExpressionKind::Break | ExpressionKind::Next => Ok(CoreType::Null),
            // `pkg::name` resolves against the seeded stub corpus by name. Whether the name really
            // belongs to `pkg` is validated by naming (which owns the corpus's namespace facts and
            // warns there); typing degrades to `Unknown` when nothing is seeded, and that gap is a
            // strict origin at this reference.
            ExpressionKind::NamespaceGet { name, .. } => {
                if let Some(binding) = self.lookup_global_name(*name) {
                    let type_scheme = binding.type_scheme.clone();
                    return self.instantiate_type_scheme(&type_scheme);
                }
                self.record_strict_origin(
                    expression.id,
                    expression.range,
                    StrictOriginKind::UndeterminedReference(*name),
                );
                Ok(CoreType::Unknown)
            }
            // `x@slot` reads an S4 slot. S4 objects are not modeled, so the read types as
            // `Unknown` and is a strict origin — but the subject is fully inferred first, so its
            // own errors surface and its variable read stays visible to naming and the IDE.
            ExpressionKind::Slot { value, .. } => {
                self.infer_expression_with_context(
                    arena.get(*value),
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
                self.record_strict_origin(
                    expression.id,
                    expression.range,
                    StrictOriginKind::UnsupportedConstruct,
                );
                Ok(CoreType::Unknown)
            }
            ExpressionKind::Unsupported => {
                self.record_strict_origin(
                    expression.id,
                    expression.range,
                    StrictOriginKind::UnsupportedConstruct,
                );
                Ok(CoreType::Unknown)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_local_variable_assign(
        &mut self,
        target: Symbol,
        value: ExpressionId,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // Variables created while inferring the assigned value live one level above
        // the binding boundary, so generalization quantifies exactly the variables
        // that do not escape into the enclosing scope, with no environment walk.
        self.enter_level();
        let binding_type_result = self.infer_assign_binding_type(
            value,
            expression,
            arena,
            resolution_context,
            type_definitions,
        );
        self.exit_level();
        // Numeric variables that escape a binding without being bound by a function
        // parameter cannot stay polymorphic, so they default to `double` here. Variables
        // reachable only inside a function type are left for generalization.
        let binding_type = self.default_free_numeric(binding_type_result?)?;
        if let Some(resolution_context) = resolution_context
            && resolution_context
                .top_level_expression_ids
                .contains(&expression.id)
        {
            let is_current_document_winner = resolution_context
                .local_naming
                .expression_resolutions
                .get(&expression.id)
                .zip(find_exported_binding(
                    resolution_context.module,
                    resolution_context.local_naming,
                    target,
                ))
                .is_some_and(|(binding_id, export_binding_id)| *binding_id == export_binding_id)
                && resolution_context
                    .package_naming
                    .global_bindings
                    .get(&target)
                    == Some(&resolution_context.document_id);

            if !is_current_document_winner {
                if let Some(binding_id) = resolution_context
                    .local_naming
                    .expression_resolutions
                    .get(&expression.id)
                    .copied()
                {
                    let generalized_scheme = self.generalize(binding_type.clone())?;
                    self.bind_local_scheme(binding_id, generalized_scheme, expression.range);
                    self.note_slot_write(
                        resolution_context.local_naming,
                        binding_id,
                        &binding_type,
                        expression,
                    )?;
                    return Ok(binding_type);
                }

                return Ok(binding_type);
            }

            self.set_environment_entry(EnvironmentKey::Global(target), None);
            let generalized_scheme = self.generalize(binding_type.clone())?;
            self.bind_global_scheme(target, generalized_scheme, expression.range);
            return Ok(binding_type);
        }

        let generalized_scheme = self.generalize(binding_type.clone())?;
        if let Some(resolution_context) = resolution_context
            && let Some(binding_id) = resolution_context
                .local_naming
                .expression_resolutions
                .get(&expression.id)
                .copied()
        {
            self.bind_local_scheme(binding_id, generalized_scheme, expression.range);
            self.note_slot_write(
                resolution_context.local_naming,
                binding_id,
                &binding_type,
                expression,
            )?;
        } else {
            self.bind_global_scheme(target, generalized_scheme, expression.range);
        }
        Ok(binding_type)
    }

    // `name <<- value`: naming resolved the write to the nearest enclosing slot (or the
    // document-scope creation slot). The write joins into that slot's environment entry as a
    // monotype — the assignment usually sits in a function body that may run never, later, or
    // repeatedly, so it can only *add* to what the slot may hold. The join is applied immediately
    // (reads later in the same body see it) and recorded as pending so it re-applies after the
    // body's environment region rolls back, making it visible from the definition site onward.
    #[allow(clippy::too_many_arguments)]
    fn infer_super_assign(
        &mut self,
        target: Symbol,
        value: ExpressionId,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        self.enter_level();
        let binding_type_result = self.infer_assign_binding_type(
            value,
            expression,
            arena,
            resolution_context,
            type_definitions,
        );
        self.exit_level();
        let binding_type = self.default_free_numeric(binding_type_result?)?;

        let Some(resolution_context) = resolution_context else {
            let generalized_scheme = self.generalize(binding_type.clone())?;
            self.bind_global_scheme(target, generalized_scheme, expression.range);
            return Ok(binding_type);
        };
        if let Some(binding_id) = resolution_context
            .local_naming
            .expression_resolutions
            .get(&expression.id)
            .copied()
        {
            let key = EnvironmentKey::Local(binding_id);
            let written = self.resolve(binding_type.clone())?;
            self.join_write_into_entry(key, written.clone(), expression)?;
            self.pending_enclosing_writes
                .push((key, written, expression.range));
            self.note_slot_write(
                resolution_context.local_naming,
                binding_id,
                &binding_type,
                expression,
            )?;
        }
        Ok(binding_type)
    }

    // A replacement assignment `x[i] <- v` / `x$a <- v` / `names(x) <- v`: reads the base
    // variable, checks every index/argument expression and the assigned value, and writes the base
    // variable's slot. The written type is the base's prior type — a replacement mutates the
    // object in place — except for a direct record field update (`x$a <- v`, `x[["a"]] <- v`),
    // which produces the record with that field set to the value's type. Element-level checking of
    // the replacement itself is not yet modeled.
    #[allow(clippy::too_many_arguments)]
    fn infer_replacement_assign(
        &mut self,
        lhs: ExpressionId,
        value: ExpressionId,
        scope: AssignmentScope,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let base = replacement_base(arena, lhs);

        // Index/argument expressions and the non-variable base position are ordinary reads.
        self.infer_replacement_lhs_parts(
            lhs,
            base.map(|(base_id, _)| base_id),
            arena,
            resolution_context,
            type_definitions,
        )?;

        self.enter_level();
        let value_type_result = self.infer_assign_binding_type(
            value,
            expression,
            arena,
            resolution_context,
            type_definitions,
        );
        self.exit_level();
        let value_type = self.default_free_numeric(value_type_result?)?;

        let Some((base_id, base_symbol)) = base else {
            // The accessor spine has no variable at its root (`f(x)$a <- v`); R rejects this shape
            // at run time, so refuse loudly rather than guess a type.
            self.record_strict_origin(
                expression.id,
                expression.range,
                StrictOriginKind::UnsupportedConstruct,
            );
            return Ok(CoreType::Unknown);
        };

        let base_expression = arena.get(base_id);
        let prior_type = self.infer_expression_with_context(
            base_expression,
            arena,
            resolution_context,
            type_definitions,
        )?;
        let prior_type = self.resolve(prior_type)?;

        let written_type =
            self.replacement_written_type(lhs, base_id, &prior_type, &value_type, arena)?;

        if let Some(resolution_context) = resolution_context
            && let Some(binding_id) = resolution_context
                .local_naming
                .expression_resolutions
                .get(&expression.id)
                .copied()
        {
            let key = EnvironmentKey::Local(binding_id);
            match scope {
                AssignmentScope::Local => {
                    self.bind_local_name(binding_id, written_type.clone(), expression.range);
                }
                AssignmentScope::Enclosing => {
                    self.join_write_into_entry(key, written_type.clone(), expression)?;
                    self.pending_enclosing_writes.push((
                        key,
                        written_type.clone(),
                        expression.range,
                    ));
                }
            }
            self.note_slot_write(
                resolution_context.local_naming,
                binding_id,
                &written_type,
                expression,
            )?;
        } else if resolution_context.is_none() {
            self.bind_global_name(base_symbol, written_type, expression.range);
        }

        // The assignment expression evaluates to the assigned value, like every other assignment.
        Ok(value_type)
    }

    // Checks the pieces of a replacement target that are ordinary reads: index and argument
    // expressions, and — when the spine has no variable root — the base position itself. The
    // callee of a replacement call is skipped (`names(x) <- v` calls `names<-`, not `names`), and
    // the base variable is skipped (its read supplies the prior type separately).
    fn infer_replacement_lhs_parts(
        &mut self,
        expression_id: ExpressionId,
        base_id: Option<ExpressionId>,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<(), InferenceError> {
        if Some(expression_id) == base_id {
            return Ok(());
        }
        let expression = arena.get(expression_id);
        match &expression.kind {
            ExpressionKind::Subset { value, arguments }
            | ExpressionKind::Subset2 { value, arguments } => {
                self.infer_replacement_lhs_parts(
                    *value,
                    base_id,
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
                for argument in arguments {
                    self.infer_expression_with_context(
                        arena.get(argument.expression),
                        arena,
                        resolution_context,
                        type_definitions,
                    )?;
                }
            }
            ExpressionKind::Dollar { value, .. } | ExpressionKind::Slot { value, .. } => {
                self.infer_replacement_lhs_parts(
                    *value,
                    base_id,
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
            }
            ExpressionKind::Call { arguments, .. } => {
                let mut argument_iter = arguments.iter();
                if let Some(first) = argument_iter.next() {
                    self.infer_replacement_lhs_parts(
                        first.expression,
                        base_id,
                        arena,
                        resolution_context,
                        type_definitions,
                    )?;
                }
                for argument in argument_iter {
                    self.infer_expression_with_context(
                        arena.get(argument.expression),
                        arena,
                        resolution_context,
                        type_definitions,
                    )?;
                }
            }
            _ => {
                self.infer_expression_with_context(
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
            }
        }
        Ok(())
    }

    // The slot type a replacement assignment writes: a direct record field update (`x$a <- v`,
    // `x[["a"]] <- v` with a literal name, applied to the base variable itself) produces the
    // record with the field set (or added); every other form keeps the base's prior type.
    fn replacement_written_type(
        &mut self,
        lhs: ExpressionId,
        base_id: ExpressionId,
        prior_type: &CoreType,
        value_type: &CoreType,
        arena: &HirArena,
    ) -> Result<CoreType, InferenceError> {
        let field_name = match &arena.get(lhs).kind {
            ExpressionKind::Dollar { value, name } if *value == base_id => Some(*name),
            ExpressionKind::Subset2 { value, arguments } if *value == base_id => {
                match arguments.as_slice() {
                    [argument] if argument.name.is_none() => {
                        match &arena.get(argument.expression).kind {
                            ExpressionKind::StringLiteralName(name) => Some(*name),
                            _ => None,
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        let (Some(field_name), CoreType::Record(fields)) = (field_name, prior_type) else {
            return Ok(prior_type.clone());
        };

        let value_type = self.resolve(value_type.clone())?;
        let mut updated_fields = fields.clone();
        match updated_fields
            .iter_mut()
            .find(|field| field.name == field_name)
        {
            Some(field) => field.value = value_type,
            None => updated_fields.push(RecordField::new(field_name, value_type)),
        }
        Ok(CoreType::Record(updated_fields))
    }

    // Joins a written type into an environment entry as a monotype (the super-assignment rule).
    // An absent entry takes the written type directly (the slot may have had no write yet).
    fn join_write_into_entry(
        &mut self,
        key: EnvironmentKey,
        written: CoreType,
        expression: &Expression,
    ) -> Result<(), InferenceError> {
        self.log_loop_access(key);
        let current = self.environment.get(&key).cloned();
        let written_binding = Binding {
            unsupplied: false,
            type_scheme: TypeScheme::monomorphic(written),
            range: expression.range,
        };
        let joined = self.join_environment_entries(current, Some(written_binding), expression)?;
        self.set_environment_entry(key, joined);
        Ok(())
    }

    // Deep-resolves a type and replaces any still-unbound inference variable with `Unknown`, for
    // values that must survive a later unification rollback (a stored variable id would dangle).
    fn erase_inference_variables(
        &mut self,
        core_type: CoreType,
    ) -> Result<CoreType, InferenceError> {
        let resolved = self.resolve(core_type)?;
        Ok(erase_variables(resolved))
    }

    fn infer_assign_binding_type(
        &mut self,
        value: ExpressionId,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let annotation = expression.annotation.as_ref();
        let value_expression = arena.get(value);

        // A checked function annotation on a function literal drives the body inference directly
        // (parameters and return are checked inside `infer_function_expression`). The binding-level
        // `apply_annotation` below would lower the annotation a second time — a fresh, conflicting set
        // of rigid binder variables — so this path returns directly and does not re-apply.
        if let ExpressionKind::Function {
            parameters,
            variadic,
            body,
        } = &value_expression.kind
            && let Some(expected_function_type) =
                self.checked_function_annotation(annotation, type_definitions, expression)?
        {
            return self.infer_function_expression(
                value_expression.id,
                parameters,
                *variadic,
                *body,
                Some(expected_function_type),
                expression,
                arena,
                resolution_context,
                type_definitions,
            );
        }

        let inferred_value = self.infer_expression_with_context(
            value_expression,
            arena,
            resolution_context,
            type_definitions,
        )?;

        if let Some(annotation) = annotation
            && annotation.applies_to_binding()
        {
            self.apply_annotation(annotation, inferred_value, expression, type_definitions)
        } else {
            Ok(inferred_value)
        }
    }

    fn infer_block(
        &mut self,
        expressions: &[ExpressionId],
        has_trailing_semicolon: bool,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        if expressions.is_empty() || has_trailing_semicolon {
            for expression_id in expressions {
                self.infer_expression_with_context(
                    arena.get(*expression_id),
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
            }
            return Ok(CoreType::Null);
        }

        let mut last_type = CoreType::Null;
        for expression_id in expressions {
            last_type = self.infer_expression_with_context(
                arena.get(*expression_id),
                arena,
                resolution_context,
                type_definitions,
            )?;
        }

        Ok(last_type)
    }

    // The trailing arena / resolution-scope / type-definition trio is the shared inference context
    // threaded through every inference method; bundling it is a separate type-checker refactor.
    #[allow(clippy::too_many_arguments)]
    fn infer_function_expression(
        &mut self,
        function_expression_id: ExpressionId,
        parameters: &[crate::hir::Parameter],
        variadic: Option<usize>,
        body: ExpressionId,
        expected_function_type: Option<FunctionType<CoreType>>,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // The body's parameter and local slot bindings live in an environment region that is rolled
        // back once the signature is inferred, so nothing the body binds leaks into the enclosing
        // scope (and the enclosing environment needs no wholesale clone).
        let pending_writes_mark = self.pending_enclosing_writes.len();
        let environment_snapshot = self.environment_snapshot();
        let signature_result = self.infer_function_signature(
            function_expression_id,
            parameters,
            variadic,
            body,
            expected_function_type.as_ref(),
            expression,
            arena,
            resolution_context,
            type_definitions,
        );
        self.environment_rollback(environment_snapshot);
        // Super-assignments in the body mutate *enclosing* slots, so their joins survive the
        // body's rollback: re-apply them here, at the function's definition site. They stay
        // recorded so each further enclosing definition site re-applies them too (the join is
        // idempotent); the list clears per top-level statement.
        let pending_writes = self.pending_enclosing_writes[pending_writes_mark..].to_vec();
        for (key, written, _) in pending_writes {
            self.join_write_into_entry(key, written, expression)?;
        }
        let inferred_function_type = signature_result?;

        // The annotation is the source of truth for the binding's interface. With the return already
        // checked, this whole-function compatibility catches parameter shape mismatches (positional
        // vs named, arity, optional vs required) and reports them against the full signature. On
        // success the binding takes the annotation's exact type, so a `<T>` binder generalizes back
        // into the declared polymorphic scheme.
        let Some(expected_function_type) = expected_function_type else {
            return Ok(CoreType::Function(inferred_function_type));
        };
        let compatible = self.check_compatibility(
            CoreType::Function(inferred_function_type.clone()),
            CoreType::Function(expected_function_type.clone()),
            type_definitions,
            Some(expression),
        )?;
        if !compatible {
            return Err(InferenceError::TypeMismatch {
                expected: Box::new(
                    self.display_with_rigid_names(&CoreType::Function(expected_function_type)),
                ),
                actual: Box::new(
                    self.display_with_rigid_names(&CoreType::Function(inferred_function_type)),
                ),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }
        Ok(CoreType::Function(expected_function_type))
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_function_signature(
        &mut self,
        function_expression_id: ExpressionId,
        parameters: &[crate::hir::Parameter],
        variadic: Option<usize>,
        body: ExpressionId,
        expected_function_type: Option<&FunctionType<CoreType>>,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<FunctionType<CoreType>, InferenceError> {
        let expected_parameter_types = match expected_function_type {
            Some(function_type) => {
                align_expected_parameter_types(function_type, parameters, expression.range)?
            }
            None => None,
        };
        let parameter_binding_ids = resolution_context.and_then(|context| {
            (!parameters.is_empty()).then(|| {
                parameters
                    .iter()
                    .map(|parameter| {
                        find_binding(
                            context.local_naming,
                            context.document_id,
                            parameter.symbol,
                            parameter.range,
                        )
                        .unwrap_or_else(|| {
                            panic!(
                                "missing parameter binding for function {:?} at {:?}",
                                function_expression_id, parameter.range
                            )
                        })
                    })
                    .collect::<Vec<_>>()
            })
        });

        let mut parameter_types = Vec::with_capacity(parameters.len());
        for (index, parameter) in parameters.iter().enumerate() {
            let parameter_type = expected_parameter_types
                .as_ref()
                .and_then(|types| types.get(index))
                .cloned()
                .unwrap_or_else(|| CoreType::Variable(self.fresh_variable()));
            if let Some(parameter_binding_ids) = &parameter_binding_ids {
                let binding_id = parameter_binding_ids
                    .get(index)
                    .copied()
                    .unwrap_or_else(|| {
                        panic!(
                            "missing parameter binding {} for function {:?}",
                            index, function_expression_id
                        )
                    });
                self.bind_local_name(binding_id, parameter_type.clone(), parameter.range);
                if let Some(context) = resolution_context {
                    self.note_slot_write(
                        context.local_naming,
                        binding_id,
                        &parameter_type,
                        expression,
                    )?;
                }
            } else {
                self.bind_global_name(parameter.symbol, parameter_type.clone(), parameter.range);
            }
            parameter_types.push(parameter_type);
        }

        // Default expressions are checked with every parameter already in scope, matching R's lazy
        // evaluation of defaults in the function frame. A `NULL` default is R's "no value" sentinel
        // for optional parameters, so it is always allowed regardless of the declared type. A
        // non-`NULL` default for an annotated parameter must be compatible with the declared type.
        // An unannotated parameter's type comes from its uses, not from its default, so a non-`NULL`
        // default does not pin it.
        for (index, parameter) in parameters.iter().enumerate() {
            let Some(default_expression_id) = parameter.default else {
                continue;
            };
            let Some(parameter_type) = parameter_types.get(index).cloned() else {
                continue;
            };
            let default_expression = arena.get(default_expression_id);
            let default_type = self.infer_expression_with_context(
                default_expression,
                arena,
                resolution_context,
                type_definitions,
            )?;
            let resolved_default = self.resolve(default_type.clone())?;
            if resolved_default == CoreType::Null {
                continue;
            }
            if expected_parameter_types
                .as_ref()
                .and_then(|types| types.get(index))
                .is_some()
            {
                self.check_argument(
                    parameter_type,
                    default_type,
                    default_expression,
                    type_definitions,
                )?;
            }
        }

        // Early returns join the trailing value: `function() { if (c) return("a"); 5 }` is
        // `character | double`. The frame is popped before the `?` so an erroring body leaves the
        // stack balanced.
        let saved_missing_narrowable = std::mem::take(&mut self.missing_narrowable);
        if let Some(parameter_binding_ids) = &parameter_binding_ids {
            for (parameter, binding_id) in parameters.iter().zip(parameter_binding_ids) {
                if parameter.default.is_none() {
                    self.missing_narrowable.insert(*binding_id);
                }
            }
        }
        self.return_type_frames.push(Vec::new());
        let body_result = self.infer_body_with_capture_discovery(
            body,
            arena,
            resolution_context,
            type_definitions,
        );
        self.missing_narrowable = saved_missing_narrowable;
        let early_returns = self
            .return_type_frames
            .pop()
            .expect("return-type frames stay balanced around body inference");
        let trailing_type = body_result?;
        let inferred_return_type = if early_returns.is_empty() {
            trailing_type
        } else {
            let resolved_trailing = self.resolve(trailing_type)?;
            let mut members = early_returns;
            members.push(resolved_trailing);
            CoreType::union_of(members)
        };

        // The body's return value only needs to be *compatible* with the annotated return (covariant,
        // like an argument against a parameter), so a body returning `integer` satisfies a declared
        // `integer | NULL` or `integer[]`. A `<T>` return is a rigid skolem, so a body returning a
        // concrete type fails here. This is checked separately to report a focused return message.
        if let Some(expected_function_type) = expected_function_type {
            let expected_return_type = (*expected_function_type.return_type).clone();
            let compatible = self.check_compatibility(
                inferred_return_type.clone(),
                expected_return_type.clone(),
                type_definitions,
                Some(expression),
            )?;
            if !compatible {
                return Err(InferenceError::TypeMismatch {
                    expected: Box::new(self.display_with_rigid_names(&expected_return_type)),
                    actual: Box::new(self.display_with_rigid_names(&inferred_return_type)),
                    range: Some(expression.range),
                    expression_id: Some(expression.id),
                });
            }
        }

        // R parameters are always matchable by name and by position, so inferred function types carry
        // every parameter as a named parameter; parameters with a default value are optional at call
        // sites, and so is a formal the body tests with `missing(name)` — R's optional-without-default
        // idiom (reads on a `missing(name)`-true edge are flow-checked and error). A `...` formal
        // becomes a rest parameter with element `Any` at its formal position (the values reaching
        // it are not tracked into the body).
        let mut missing_tested = std::collections::BTreeSet::new();
        if let Some(missing_symbol) = self.missing_symbol {
            crate::hir::formals_tested_by_missing(arena, body, missing_symbol, &mut missing_tested);
        }
        let named_parameter_types = parameters
            .iter()
            .zip(parameter_types)
            .map(|(parameter, parameter_type)| {
                RecordField::with_optional(
                    parameter.symbol,
                    parameter_type,
                    parameter.has_default() || missing_tested.contains(&parameter.symbol),
                )
            })
            .collect();
        Ok(FunctionType::with_variadic(
            Vec::new(),
            named_parameter_types,
            variadic.map(|preceding_named| RestParameter {
                element: Box::new(CoreType::Any),
                preceding_named,
            }),
            inferred_return_type,
        ))
    }

    // Checks a function body, re-running it once when the walk wrote a captured slot flagged for
    // the discovery re-pass (`capture_repass_slots`): the first run exists to complete the frame's
    // captured-write joins and is then fully discarded — environment, unification, strict origins,
    // and pending super-assign writes all roll back — so the second run resolves captured reads
    // against the completed joins with no stale effects. Bodies that never write such a slot (the
    // overwhelming majority) pay only two snapshot markers.
    fn infer_body_with_capture_discovery(
        &mut self,
        body: ExpressionId,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let saved_wrote_repass_slot = std::mem::replace(&mut self.wrote_repass_slot, false);
        let environment_snapshot = self.environment_snapshot();
        let unification_snapshot = self.snapshot();
        let origins_mark = self.strict_origins.len();
        let pending_writes_mark = self.pending_enclosing_writes.len();
        let discovery = self.infer_expression_with_context(
            arena.get(body),
            arena,
            resolution_context,
            type_definitions,
        );
        if !self.wrote_repass_slot {
            self.environment_commit(environment_snapshot);
            self.commit(unification_snapshot);
            self.wrote_repass_slot = saved_wrote_repass_slot;
            return discovery;
        }

        self.environment_rollback(environment_snapshot);
        self.rollback_to(unification_snapshot);
        self.strict_origins.truncate(origins_mark);
        self.pending_enclosing_writes.truncate(pending_writes_mark);
        // The write-join table deliberately survives (its entries carry no inference variables);
        // memoized loop exits do not — their types may reference just-reclaimed variable ids.
        self.loop_memos.clear();
        let result = self.infer_expression_with_context(
            arena.get(body),
            arena,
            resolution_context,
            type_definitions,
        );
        // The re-pass wrote the flagged slot again; keep the flag set so enclosing frames that may
        // own the slot re-run their own discovery.
        self.wrote_repass_slot |= saved_wrote_repass_slot;
        result
    }

    fn apply_annotation(
        &mut self,
        annotation: &AttachedAnnotation,
        inferred_type: CoreType,
        expression: &Expression,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        match annotation.annotation() {
            Annotation::Type { kind, surface_type } => {
                let actual_type = self.resolve(inferred_type)?;
                // Naming already diagnosed an unresolved or misapplied type name in the
                // annotation; checking the value against it would only cascade noise.
                let expected_type = match self.lower_annotation_surface_type(
                    surface_type,
                    type_definitions,
                    Some(expression),
                ) {
                    Ok(expected_type) => expected_type,
                    Err(InferenceError::UnresolvedAnnotationType { .. }) => {
                        return Ok(actual_type);
                    }
                    Err(error) => return Err(error),
                };

                match kind {
                    TypeAnnotationKind::Checked => {
                        if self.check_compatibility(
                            actual_type.clone(),
                            expected_type.clone(),
                            type_definitions,
                            Some(expression),
                        )? {
                            Ok(expected_type)
                        } else {
                            // The check failed and its speculative bindings were already reverted by
                            // the `check_compatibility` wrapper. Re-running unification can surface a
                            // more specific cause (occurs check, constraint violation, arity) than the
                            // bare `TypeMismatch`; run it inside a snapshot that is always rolled back
                            // so this error extraction leaves no net mutation.
                            let snapshot = self.snapshot();
                            let unify_result = self.unify_with_context(
                                expected_type.clone(),
                                actual_type.clone(),
                                expression,
                            );
                            self.rollback_to(snapshot);
                            match unify_result {
                                Err(error) => Err(error),
                                Ok(_) => Err(InferenceError::TypeMismatch {
                                    expected: Box::new(expected_type),
                                    actual: Box::new(actual_type),
                                    range: Some(expression.range),
                                    expression_id: Some(expression.id),
                                }),
                            }
                        }
                    }
                    TypeAnnotationKind::UnknownOnly => {
                        if actual_type == CoreType::Unknown {
                            Ok(expected_type)
                        } else {
                            Err(InferenceError::TypeMismatch {
                                expected: Box::new(CoreType::Unknown),
                                actual: Box::new(actual_type),
                                range: Some(expression.range),
                                expression_id: Some(expression.id),
                            })
                        }
                    }
                    TypeAnnotationKind::Trusted => Ok(expected_type),
                }
            }
            Annotation::New { nominal_type } => {
                let lowered_arguments = match nominal_type
                    .type_arguments
                    .iter()
                    .map(|argument| {
                        self.lower_annotation_surface_type(
                            argument,
                            type_definitions,
                            Some(expression),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(lowered_arguments) => lowered_arguments,
                    Err(InferenceError::UnresolvedAnnotationType { .. }) => {
                        return self.resolve(inferred_type);
                    }
                    Err(error) => return Err(error),
                };

                // Naming already diagnoses `@new` on unknown names, aliases, and wrong type
                // argument arity; typecheck recovers without piling on a second diagnostic.
                let is_nominal_definition = type_definitions
                    .get(nominal_type.name)
                    .is_some_and(|definition| definition.kind == DefinitionKind::Type);
                if !is_nominal_definition {
                    return self.resolve(inferred_type);
                }

                let Some(representation_type) = self.nominal_representation_type(
                    nominal_type.name,
                    &lowered_arguments,
                    type_definitions,
                    Some(expression),
                )?
                else {
                    return self.resolve(inferred_type);
                };

                let actual_type = self.resolve(inferred_type)?;
                if self.check_compatibility(
                    actual_type.clone(),
                    representation_type.clone(),
                    type_definitions,
                    Some(expression),
                )? {
                    Ok(CoreType::Nominal(nominal_type.name, lowered_arguments))
                } else {
                    Err(InferenceError::TypeMismatch {
                        expected: Box::new(representation_type),
                        actual: Box::new(actual_type),
                        range: Some(expression.range),
                        expression_id: Some(expression.id),
                    })
                }
            }
        }
    }

    fn checked_function_annotation(
        &mut self,
        annotation: Option<&AttachedAnnotation>,
        type_definitions: &TypeDefinitionEnvironment,
        expression: &Expression,
    ) -> Result<Option<FunctionType<CoreType>>, InferenceError> {
        let Some(annotation) = annotation else {
            return Ok(None);
        };
        Ok(match annotation.annotation() {
            Annotation::Type {
                kind: TypeAnnotationKind::Checked,
                surface_type,
            } => match self.lower_annotation_surface_type(
                surface_type,
                type_definitions,
                Some(expression),
            ) {
                Ok(CoreType::Function(function_type)) => Some(function_type),
                Ok(_) | Err(InferenceError::UnresolvedAnnotationType { .. }) => None,
                Err(error) => return Err(error),
            },
            Annotation::Type { .. } | Annotation::New { .. } => None,
        })
    }

    fn lower_annotation_surface_type(
        &mut self,
        surface_type: &SurfaceType,
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        self.lower_surface_type_with_substitutions(
            surface_type,
            &BTreeMap::new(),
            &mut BTreeSet::new(),
            type_definitions,
            expression,
        )
    }

    // How a lowered `T[]` / `T[named]` element becomes a core type. A concrete atomic or a
    // statically untracked element (`Any`/`Unknown`) forms a vector directly. A type *variable*
    // element — a `<T>` binder used as `T[]` — also forms a vector, and the variable acquires the
    // atomic-element bound. The bound is recorded straight on the entry (not via `constrain_type`)
    // because the element may be a rigid binder: the annotation itself makes the atomic promise
    // here, unlike a function body, which must not add bounds the annotation never declared. Every
    // other element shape is refused: vectors hold atomic elements only, and the historical silent
    // reading of `X[]` as `list[X]` hid the mistake.
    fn lower_vector_element(
        &mut self,
        element: CoreType,
        vector: impl Fn(Box<CoreType>) -> CoreType,
    ) -> Result<CoreType, InferenceError> {
        match element {
            CoreType::Scalar(_) | CoreType::Any | CoreType::Unknown => {
                Ok(vector(Box::new(element)))
            }
            CoreType::Variable(variable) => {
                if let Some(InferenceEntry::Unbound { level, constraint }) =
                    self.entries.get(&variable)
                {
                    let raised = InferenceEntry::Unbound {
                        level: *level,
                        constraint: constraint.join(Constraint::AtomicElement),
                    };
                    self.set_entry(variable, raised);
                }
                Ok(vector(Box::new(CoreType::Variable(variable))))
            }
            other_type => Err(InferenceError::InvalidVectorElement {
                element: Box::new(other_type),
            }),
        }
    }

    fn lower_surface_type_with_substitutions(
        &mut self,
        surface_type: &SurfaceType,
        substitutions: &BTreeMap<Symbol, CoreType>,
        expanding_aliases: &mut BTreeSet<Symbol>,
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        // Lowering can recurse deeper than the parsed annotation when an alias body expands (see the
        // `SurfaceType::Named` alias arm), so it carries its own guard rather than relying on the
        // type-syntax parser's depth bound.
        if self.recursion_depth >= RECURSION_LIMIT {
            return Err(InferenceError::RecursionLimitExceeded);
        }
        self.recursion_depth += 1;
        let result = self.lower_surface_type_with_substitutions_inner(
            surface_type,
            substitutions,
            expanding_aliases,
            type_definitions,
            expression,
        );
        self.recursion_depth -= 1;
        result
    }

    fn lower_surface_type_with_substitutions_inner(
        &mut self,
        surface_type: &SurfaceType,
        substitutions: &BTreeMap<Symbol, CoreType>,
        expanding_aliases: &mut BTreeSet<Symbol>,
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        match surface_type {
            SurfaceType::Any => Ok(CoreType::Any),
            SurfaceType::Unknown => Ok(CoreType::Unknown),
            SurfaceType::Null => Ok(CoreType::Null),
            SurfaceType::Union(members) => {
                let lowered = members
                    .iter()
                    .map(|member| {
                        self.lower_surface_type_with_substitutions(
                            member,
                            substitutions,
                            expanding_aliases,
                            type_definitions,
                            expression,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                // Re-normalize: an alias member may have expanded into a type equal to another
                // member, or into a union itself.
                Ok(CoreType::union_of(lowered))
            }
            SurfaceType::Scalar(atomic) => Ok(CoreType::Scalar(*atomic)),
            SurfaceType::Named(name, arguments) => {
                if let Some(core_type) = substitutions.get(name) {
                    if arguments.is_empty() {
                        return Ok(core_type.clone());
                    }
                    // Applying type arguments to a type parameter is a naming diagnostic; lowering
                    // through the (shadowed) global of the same name would silently resolve the
                    // misuse, so it degrades to the same silent skip a wrong arity gets.
                    return Err(InferenceError::UnresolvedAnnotationType { symbol: *name });
                }

                let lowered_arguments = arguments
                    .iter()
                    .map(|argument| {
                        self.lower_surface_type_with_substitutions(
                            argument,
                            substitutions,
                            expanding_aliases,
                            type_definitions,
                            expression,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let Some(type_definition) = type_definitions.get(*name).cloned() else {
                    return Err(InferenceError::UnresolvedAnnotationType { symbol: *name });
                };

                // A wrong type-argument count is already a naming diagnostic; lowering the
                // misapplication anyway would check the value against a malformed type and
                // cascade a second error, so it degrades to the same silent skip an unresolved
                // name gets.
                if type_definition.type_parameters.len() != lowered_arguments.len() {
                    return Err(InferenceError::UnresolvedAnnotationType { symbol: *name });
                }

                match type_definition.kind {
                    DefinitionKind::Type => Ok(CoreType::Nominal(*name, lowered_arguments)),
                    DefinitionKind::Alias => {
                        if !expanding_aliases.insert(*name) {
                            return Err(alias_cycle_error(*name, expression));
                        }

                        let lowered_alias = if type_definition.type_parameters.len()
                            != lowered_arguments.len()
                        {
                            Err(InferenceError::UnresolvedAnnotationType { symbol: *name })
                        } else {
                            let mut nested_substitutions = substitutions.clone();
                            for (type_parameter, lowered_argument) in type_definition
                                .type_parameters
                                .iter()
                                .zip(lowered_arguments)
                            {
                                nested_substitutions.insert(*type_parameter, lowered_argument);
                            }

                            match &type_definition.representation {
                                Some(representation) => self.lower_surface_type_with_substitutions(
                                    representation,
                                    &nested_substitutions,
                                    expanding_aliases,
                                    type_definitions,
                                    expression,
                                ),
                                // An alias always carries a representation; an opaque
                                // definition cannot be expanded.
                                None => {
                                    Err(InferenceError::UnresolvedAnnotationType { symbol: *name })
                                }
                            }
                        };

                        expanding_aliases.remove(name);
                        lowered_alias
                    }
                }
            }
            SurfaceType::Vector(inner_type) => {
                let element = self.lower_surface_type_with_substitutions(
                    inner_type,
                    substitutions,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )?;
                self.lower_vector_element(element, CoreType::Vector)
            }
            SurfaceType::NamedVector(inner_type) => {
                let element = self.lower_surface_type_with_substitutions(
                    inner_type,
                    substitutions,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )?;
                self.lower_vector_element(element, CoreType::NamedVector)
            }
            SurfaceType::List(item_type) => Ok(CoreType::List(Box::new(
                self.lower_surface_type_with_substitutions(
                    item_type,
                    substitutions,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )?,
            ))),
            SurfaceType::NamedList(item_type) => Ok(CoreType::NamedList(Box::new(
                self.lower_surface_type_with_substitutions(
                    item_type,
                    substitutions,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )?,
            ))),
            SurfaceType::Record(fields) => Ok(CoreType::Record(
                fields
                    .iter()
                    .map(|field| {
                        Ok(RecordField::with_optional(
                            field.name,
                            self.lower_surface_type_with_substitutions(
                                &field.value,
                                substitutions,
                                expanding_aliases,
                                type_definitions,
                                expression,
                            )?,
                            field.optional,
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            SurfaceType::Tuple(items) => Ok(CoreType::Tuple(
                items
                    .iter()
                    .map(|item| {
                        self.lower_surface_type_with_substitutions(
                            item,
                            substitutions,
                            expanding_aliases,
                            type_definitions,
                            expression,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            SurfaceType::Function(function_type) => {
                let variadic = function_type
                    .variadic
                    .as_ref()
                    .map(|variadic| {
                        Ok::<_, InferenceError>(RestParameter {
                            element: Box::new(self.lower_surface_type_with_substitutions(
                                &variadic.element,
                                substitutions,
                                expanding_aliases,
                                type_definitions,
                                expression,
                            )?),
                            preceding_named: variadic.preceding_named,
                        })
                    })
                    .transpose()?;
                Ok(CoreType::Function(FunctionType::with_variadic(
                    function_type
                        .parameters
                        .iter()
                        .map(|parameter| {
                            self.lower_surface_type_with_substitutions(
                                parameter,
                                substitutions,
                                expanding_aliases,
                                type_definitions,
                                expression,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    function_type
                        .named_parameters
                        .iter()
                        .map(|parameter| {
                            Ok(RecordField::with_optional(
                                parameter.name,
                                self.lower_surface_type_with_substitutions(
                                    &parameter.value,
                                    substitutions,
                                    expanding_aliases,
                                    type_definitions,
                                    expression,
                                )?,
                                parameter.optional,
                            ))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    variadic,
                    self.lower_surface_type_with_substitutions(
                        &function_type.return_type,
                        substitutions,
                        expanding_aliases,
                        type_definitions,
                        expression,
                    )?,
                )))
            }
            SurfaceType::Binders(bound_type_parameters, inner_type) => {
                if bound_type_parameters.is_empty() {
                    return self.lower_surface_type_with_substitutions(
                        inner_type,
                        substitutions,
                        expanding_aliases,
                        type_definitions,
                        expression,
                    );
                }

                // A `<T>` binder introduces a universally quantified parameter. While checking a
                // function body against the annotation it must be rigid, so the body cannot bind or
                // constrain it (the body has to work for every T); after the check it generalizes
                // back into the scheme. Instantiating a stored scheme uses ordinary fresh variables,
                // so this only makes annotation binders rigid.
                let mut nested_type_parameters = substitutions.clone();
                for type_parameter in bound_type_parameters {
                    // A declared constraint (`<T: numeric>`) rides on the rigid variable from
                    // creation: the annotation itself promises it, so the body may use the
                    // parameter under that bound and the scheme generalizes back with it.
                    let variable =
                        self.fresh_rigid_variable(type_parameter.name, type_parameter.constraint);
                    nested_type_parameters
                        .insert(type_parameter.name, CoreType::Variable(variable));
                }

                self.lower_surface_type_with_substitutions(
                    inner_type,
                    &nested_type_parameters,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )
            }
        }
    }

    fn nominal_representation_type(
        &mut self,
        symbol: Symbol,
        type_arguments: &[CoreType],
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<Option<CoreType>, InferenceError> {
        let Some(type_definition) = type_definitions.get(symbol).cloned() else {
            return Ok(None);
        };
        if type_definition.kind != DefinitionKind::Type {
            return Ok(None);
        }

        if type_definition.type_parameters.len() != type_arguments.len() {
            return Ok(None);
        }

        let mut substitutions = BTreeMap::new();
        for (type_parameter, type_argument) in type_definition
            .type_parameters
            .iter()
            .zip(type_arguments.iter())
        {
            substitutions.insert(*type_parameter, type_argument.clone());
        }

        let Some(representation) = &type_definition.representation else {
            return Ok(None);
        };
        match self.lower_surface_type_with_substitutions(
            &representation.clone(),
            &substitutions,
            &mut BTreeSet::new(),
            type_definitions,
            expression,
        ) {
            Ok(representation_type) => Ok(Some(representation_type)),
            Err(InferenceError::UnresolvedAnnotationType { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn infer_builtin_call(
        &mut self,
        symbol: Symbol,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<Option<CoreType>, InferenceError> {
        let Some(builtin_kind) = self.builtins.get(&symbol).copied() else {
            return Ok(None);
        };

        match builtin_kind {
            BuiltinKind::Modulo | BuiltinKind::IntegerDivide => self
                .infer_binary_numeric(
                    arguments,
                    expression,
                    NumericResultAtomic::Promote,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Colon => self
                .infer_builtin_colon(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Compare => self
                .infer_builtin_compare(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Plus => self
                .infer_builtin_plus(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Minus => self
                .infer_builtin_minus(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Multiply => self
                .infer_builtin_multiply(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Divide => self
                .infer_builtin_divide(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Power => self
                .infer_builtin_power(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::And => self
                .infer_builtin_boolean_binary(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Or => self
                .infer_builtin_boolean_binary(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Combine => self
                .infer_builtin_combine(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::List => self
                .infer_builtin_list(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
            BuiltinKind::Switch => self
                .infer_builtin_switch(
                    arguments,
                    expression,
                    arena,
                    resolution_context,
                    type_definitions,
                )
                .map(Some),
        }
    }

    fn infer_builtin_plus(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        self.infer_binary_numeric(
            arguments,
            expression,
            NumericResultAtomic::Promote,
            arena,
            resolution_context,
            type_definitions,
        )
    }

    fn infer_builtin_minus(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        self.infer_binary_numeric(
            arguments,
            expression,
            NumericResultAtomic::Promote,
            arena,
            resolution_context,
            type_definitions,
        )
    }

    fn infer_builtin_multiply(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        self.infer_binary_numeric(
            arguments,
            expression,
            NumericResultAtomic::Promote,
            arena,
            resolution_context,
            type_definitions,
        )
    }

    fn infer_builtin_divide(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        self.infer_binary_numeric(
            arguments,
            expression,
            NumericResultAtomic::AlwaysDouble,
            arena,
            resolution_context,
            type_definitions,
        )
    }

    fn infer_builtin_power(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        self.infer_binary_numeric(
            arguments,
            expression,
            NumericResultAtomic::AlwaysDouble,
            arena,
            resolution_context,
            type_definitions,
        )
    }

    fn infer_binary_numeric(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        numeric_result_atomic: NumericResultAtomic,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        if arguments.len() != 2 {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 2,
                actual: arguments.len(),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        let arg0 = arena.get(arguments[0].expression);
        let arg1 = arena.get(arguments[1].expression);

        let left_type =
            self.infer_expression_with_context(arg0, arena, resolution_context, type_definitions)?;
        let right_type =
            self.infer_expression_with_context(arg1, arena, resolution_context, type_definitions)?;

        let resolved_left = self.resolve_structural(left_type, type_definitions, Some(arg0))?;
        let resolved_right = self.resolve_structural(right_type, type_definitions, Some(arg1))?;

        let left = classify_numeric_operand(&resolved_left);
        let right = classify_numeric_operand(&resolved_right);

        if let NumericOperand::Invalid = left {
            return Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Numeric,
                actual: Box::new(resolved_left),
                range: arg0.range,
                expression_id: arg0.id,
            });
        }
        if let NumericOperand::Invalid = right {
            return Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Numeric,
                actual: Box::new(resolved_right),
                range: arg1.range,
                expression_id: arg1.id,
            });
        }
        if matches!(left, NumericOperand::AnyUnknown) || matches!(right, NumericOperand::AnyUnknown)
        {
            return Ok(CoreType::Unknown);
        }

        // Constrain every flexible operand to be numeric, collapsing them onto one representative
        // variable so `x + y` ties the two operands together.
        let mut flexible_variable: Option<InferenceVariableId> = None;
        for operand in [&left, &right] {
            if let NumericOperand::Variable(variable) = operand {
                flexible_variable = Some(match flexible_variable {
                    Some(existing) => match self
                        .unify(CoreType::Variable(existing), CoreType::Variable(*variable))?
                    {
                        CoreType::Variable(unified) => unified,
                        _ => existing,
                    },
                    None => *variable,
                });
            }
        }
        if let Some(variable) = flexible_variable {
            self.constrain_type(
                CoreType::Variable(variable),
                Constraint::Numeric,
                Some(expression),
            )?;
        }

        // A generic vector element (`T[]`) used arithmetically must be numeric; joined with the
        // atomic-element bound it already carries, the element becomes scalar-numeric.
        for operand in [&left, &right] {
            if let NumericOperand::FlexibleVector(Some(element_variable)) = operand {
                self.constrain_type(
                    CoreType::Variable(*element_variable),
                    Constraint::Numeric,
                    Some(expression),
                )?;
            }
        }

        // A flexible-element vector operand fixes the result shape (vector) without fixing the
        // atomic. Mirroring the scalar flexible-operand rules: an always-double operation or a
        // concrete `double` (or union) partner promotes to `double[]`; an integer partner promotes
        // *into* the element, so the result keeps the element variable; two generic elements are
        // unified; an untracked (`Any`/`Unknown`) element stays untracked.
        let flexible_vector_present = matches!(left, NumericOperand::FlexibleVector(_))
            || matches!(right, NumericOperand::FlexibleVector(_));
        if flexible_vector_present {
            if let NumericResultAtomic::AlwaysDouble = numeric_result_atomic {
                return Ok(CoreType::vector(Atomic::Double));
            }
            let concrete_parts = left.concrete_parts().or_else(|| right.concrete_parts());
            if let Some(parts) = &concrete_parts
                && (parts.len() > 1 || parts.iter().any(|(_, atomic)| *atomic == Atomic::Double))
            {
                return Ok(CoreType::vector(Atomic::Double));
            }
            let element = match (&left, &right) {
                (
                    NumericOperand::FlexibleVector(Some(left_element)),
                    NumericOperand::FlexibleVector(Some(right_element)),
                ) => Some(self.unify(
                    CoreType::Variable(*left_element),
                    CoreType::Variable(*right_element),
                )?),
                (NumericOperand::FlexibleVector(None), _)
                | (_, NumericOperand::FlexibleVector(None)) => None,
                (NumericOperand::FlexibleVector(Some(element)), _)
                | (_, NumericOperand::FlexibleVector(Some(element))) => {
                    Some(CoreType::Variable(*element))
                }
                _ => None,
            };
            return Ok(CoreType::Vector(Box::new(
                element.unwrap_or(CoreType::Unknown),
            )));
        }

        match (left.concrete_parts(), right.concrete_parts()) {
            // Member-wise: the operation applies to every pair of operand members, and the result
            // is the join of the per-pair results. A single concrete operand is the one-member
            // case, so this arm also carries the ordinary concrete/concrete path: both-`integer`
            // pairs stay `integer`, any `double` promotes the pair, and a vector member makes the
            // pair's result a vector.
            (Some(left_parts), Some(right_parts)) => Ok(CoreType::union_of(
                member_wise_numeric_results(&left_parts, &right_parts, numeric_result_atomic),
            )),
            (left_parts, right_parts) => {
                let variable = flexible_variable
                    .expect("a non-concrete numeric operand classifies as a variable");
                let concrete_parts = left_parts.or(right_parts);
                if let Some(parts) = &concrete_parts
                    && parts.len() > 1
                {
                    // A union operand cannot promote into a variable member-wise, so the flexible
                    // side is pinned to the default numeric scalar (`double`) — the same default a
                    // vector result applies below — and the operation continues member-wise.
                    self.bind_variable(
                        variable,
                        CoreType::Scalar(Atomic::Double),
                        Some(expression),
                    )?;
                    return Ok(CoreType::union_of(member_wise_numeric_results(
                        &[(OperandShape::Scalar, Atomic::Double)],
                        parts,
                        numeric_result_atomic,
                    )));
                }

                let concrete = concrete_parts.and_then(|parts| parts.first().copied());
                let result_shape = match concrete {
                    Some((OperandShape::Vector, _)) => OperandShape::Vector,
                    _ => OperandShape::Scalar,
                };
                if let NumericResultAtomic::AlwaysDouble = numeric_result_atomic {
                    return Ok(core_type_for_shape(result_shape, Atomic::Double));
                }
                // Promote: a concrete `double` anywhere forces `double`.
                if concrete.map(|(_, atomic)| atomic) == Some(Atomic::Double) {
                    return Ok(core_type_for_shape(result_shape, Atomic::Double));
                }
                match result_shape {
                    // `x + 1L` (and `x + y`) stay polymorphic over the numeric operand: integer
                    // promotes to whatever the variable resolves to, so the scalar result is the
                    // variable itself.
                    OperandShape::Scalar => Ok(CoreType::Variable(variable)),
                    // A vector result cannot carry an unresolved atomic, so a flexible operand
                    // defaults to `double` here.
                    OperandShape::Vector => {
                        self.bind_variable(
                            variable,
                            CoreType::Scalar(Atomic::Double),
                            Some(expression),
                        )?;
                        Ok(CoreType::vector(Atomic::Double))
                    }
                }
            }
        }
    }

    fn infer_unary_minus(
        &mut self,
        value: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let inferred_type =
            self.infer_expression_with_context(value, arena, resolution_context, type_definitions)?;
        let resolved_type =
            self.resolve_structural(inferred_type, type_definitions, Some(value))?;

        match classify_numeric_operand(&resolved_type) {
            NumericOperand::Concrete(shape, atomic) => Ok(core_type_for_shape(shape, atomic)),
            // Member-wise over a union operand: negation preserves each member's shape and atomic,
            // so the result is the same union.
            NumericOperand::ConcreteUnion(parts) => Ok(CoreType::union_of(
                parts
                    .into_iter()
                    .map(|(shape, atomic)| core_type_for_shape(shape, atomic))
                    .collect(),
            )),
            NumericOperand::Variable(variable) => {
                self.constrain_type(
                    CoreType::Variable(variable),
                    Constraint::Numeric,
                    Some(value),
                )?;
                Ok(CoreType::Variable(variable))
            }
            // Negation is elementwise and type-preserving, so a generic-element vector keeps its
            // element (constrained numeric) and an untracked element stays untracked.
            NumericOperand::FlexibleVector(element_variable) => {
                if let Some(element_variable) = element_variable {
                    self.constrain_type(
                        CoreType::Variable(element_variable),
                        Constraint::Numeric,
                        Some(value),
                    )?;
                }
                Ok(resolved_type)
            }
            NumericOperand::AnyUnknown => Ok(CoreType::Unknown),
            NumericOperand::Invalid => Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Numeric,
                actual: Box::new(resolved_type),
                range: value.range,
                expression_id: value.id,
            }),
        }
    }

    fn infer_unary_not(
        &mut self,
        value: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let inferred_type =
            self.infer_expression_with_context(value, arena, resolution_context, type_definitions)?;
        let resolved_type =
            self.resolve_structural(inferred_type, type_definitions, Some(value))?;

        match resolved_type {
            CoreType::Scalar(Atomic::Logical) => Ok(CoreType::Scalar(Atomic::Logical)),
            CoreType::Vector(ref element) | CoreType::NamedVector(ref element)
                if matches!(
                    element.as_ref(),
                    CoreType::Scalar(Atomic::Logical)
                        | CoreType::Variable(_)
                        | CoreType::Any
                        | CoreType::Unknown
                ) =>
            {
                Ok(CoreType::vector(Atomic::Logical))
            }
            CoreType::Any | CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Variable(_) => {
                self.unify_with_context(CoreType::Scalar(Atomic::Logical), resolved_type, value)?;
                Ok(CoreType::Scalar(Atomic::Logical))
            }
            other_type => Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Logical,
                actual: Box::new(other_type),
                range: value.range,
                expression_id: value.id,
            }),
        }
    }

    fn infer_builtin_compare(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        if arguments.len() != 2 {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 2,
                actual: arguments.len(),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        let arg0 = arena.get(arguments[0].expression);
        let arg1 = arena.get(arguments[1].expression);
        let left_type =
            self.infer_expression_with_context(arg0, arena, resolution_context, type_definitions)?;
        let right_type =
            self.infer_expression_with_context(arg1, arena, resolution_context, type_definitions)?;
        let resolved_left = self.resolve_structural(left_type, type_definitions, Some(arg0))?;
        let resolved_right = self.resolve_structural(right_type, type_definitions, Some(arg1))?;

        if matches!(resolved_left, CoreType::Any | CoreType::Unknown)
            || matches!(resolved_right, CoreType::Any | CoreType::Unknown)
        {
            return Ok(CoreType::Unknown);
        }

        let left_parts = comparison_operand_parts_list(&resolved_left);
        let right_parts = comparison_operand_parts_list(&resolved_right);
        let left_flexible = flexible_comparison_operand(&resolved_left);
        let right_flexible = flexible_comparison_operand(&resolved_right);
        let left_is_variable = left_flexible.is_some();
        let right_is_variable = right_flexible.is_some();

        if left_parts.is_none() && !left_is_variable {
            return Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Comparable,
                actual: Box::new(resolved_left),
                range: arg0.range,
                expression_id: arg0.id,
            });
        }
        if right_parts.is_none() && !right_is_variable {
            return Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Comparable,
                actual: Box::new(resolved_right),
                range: arg1.range,
                expression_id: arg1.id,
            });
        }

        // Two concrete operands must belong to the same comparison family, member-wise: every
        // shape the left union can take must be comparable with every shape of the right.
        if let (Some(left_parts), Some(right_parts)) = (&left_parts, &right_parts)
            && left_parts.iter().any(|(_, left_family)| {
                right_parts
                    .iter()
                    .any(|(_, right_family)| left_family != right_family)
            })
        {
            return Err(InferenceError::TypeMismatch {
                expected: Box::new(resolved_left),
                actual: Box::new(resolved_right),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        // A flexible operand compared against a concrete numeric operand is constrained numeric;
        // comparison against a non-numeric family leaves it free, since the type system has no
        // character-or-logical constraint.
        let all_numeric = |parts: &Option<Vec<(OperandShape, ComparisonFamily)>>| {
            parts.as_ref().is_some_and(|parts| {
                parts
                    .iter()
                    .all(|(_, family)| *family == ComparisonFamily::Numeric)
            })
        };
        if let Some(flexible) = &left_flexible
            && all_numeric(&right_parts)
            && let Some(variable) = flexible.variable()
        {
            self.constrain_type(
                CoreType::Variable(variable),
                Constraint::Numeric,
                Some(arg0),
            )?;
        }
        if let Some(flexible) = &right_flexible
            && all_numeric(&left_parts)
            && let Some(variable) = flexible.variable()
        {
            self.constrain_type(
                CoreType::Variable(variable),
                Constraint::Numeric,
                Some(arg1),
            )?;
        }

        // Member-wise result: a pair with a vector member compares element-wise (`logical[]`), a
        // scalar-scalar pair compares to `logical`; a union operand mixing shapes therefore yields
        // the join of both. A flexible-element vector operand has no concrete parts but a known
        // vector shape.
        let left_shapes = shapes_for_operand(&left_parts, &left_flexible);
        let right_shapes = shapes_for_operand(&right_parts, &right_flexible);
        let mut results = Vec::new();
        for left_shape in &left_shapes {
            for right_shape in &right_shapes {
                let result_shape = if *left_shape == OperandShape::Vector
                    || *right_shape == OperandShape::Vector
                {
                    OperandShape::Vector
                } else {
                    OperandShape::Scalar
                };
                results.push(core_type_for_shape(result_shape, Atomic::Logical));
            }
        }
        Ok(CoreType::union_of(results))
    }

    fn infer_builtin_colon(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        if arguments.len() != 2 {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 2,
                actual: arguments.len(),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        let mut result_atomic = Atomic::Integer;
        for argument in arguments {
            let argument_expression = arena.get(argument.expression);
            let inferred_argument = self.infer_expression_with_context(
                argument_expression,
                arena,
                resolution_context,
                type_definitions,
            )?;
            let resolved_argument = self.resolve_structural(
                inferred_argument,
                type_definitions,
                Some(argument_expression),
            )?;
            match resolved_argument {
                CoreType::Scalar(Atomic::Integer) => {}
                // R's `:` yields an integer sequence for whole-number endpoints, so
                // whole-number double literals like `1` in `1:10` count as integer here.
                CoreType::Scalar(Atomic::Double)
                    if is_whole_number_double_literal(argument_expression) => {}
                CoreType::Scalar(Atomic::Double) => result_atomic = Atomic::Double,
                CoreType::Any | CoreType::Unknown => return Ok(CoreType::Unknown),
                // A flexible endpoint such as `1:n` must be a scalar number — the plain numeric
                // bound admits numeric vectors, which R only warns about and truncates to the
                // first element. It is also not known to be `integer`; it may resolve to `double`,
                // so the result must be `double[]` (claiming `integer[]` would be unsound when the
                // endpoint instantiates at `double`).
                CoreType::Variable(variable) => {
                    self.constrain_type(
                        CoreType::Variable(variable),
                        Constraint::ScalarNumeric,
                        Some(argument_expression),
                    )?;
                    result_atomic = Atomic::Double;
                }
                other_type => {
                    return Err(InferenceError::InvalidOperand {
                        expected: OperandExpectation::ScalarNumeric,
                        actual: Box::new(other_type),
                        range: argument_expression.range,
                        expression_id: argument_expression.id,
                    });
                }
            }
        }

        Ok(CoreType::vector(result_atomic))
    }

    fn infer_subset_expression(
        &mut self,
        value: &Expression,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // The subject and every index are inferred first regardless of shape, so names inside an
        // unsupported form (`m[i, j]`) still resolve and get their own diagnostics.
        let inferred_value =
            self.infer_expression_with_context(value, arena, resolution_context, type_definitions)?;
        let value_type = self.resolve_structural(inferred_value, type_definitions, Some(value))?;
        for argument in arguments {
            let argument_expression = arena.get(argument.expression);
            self.infer_expression_with_context(
                argument_expression,
                arena,
                resolution_context,
                type_definitions,
            )?;
        }

        // An Unknown/Any subject stays Unknown/Any even under an unsupported index shape — the
        // subject's own gap was already diagnosed, so `m[i, j]` must not cascade an arity error.
        if matches!(value_type, CoreType::Unknown) {
            return Ok(CoreType::Unknown);
        }
        if matches!(value_type, CoreType::Any) {
            return Ok(CoreType::Any);
        }
        // A sealed nominal supports value-dependent indexing of any shape at runtime
        // (`df[rows, cols]`, `df[predicate, ]`), none of it modeled: `Unknown`, before the
        // index-arity check, so idiomatic two-index data.frame subsetting is not an error.
        if matches!(value_type, CoreType::Nominal(..)) {
            self.record_strict_origin(
                expression.id,
                expression.range,
                StrictOriginKind::UnsupportedConstruct,
            );
            return Ok(CoreType::Unknown);
        }
        if arguments.len() != 1 || arguments[0].name.is_some() {
            return Err(InferenceError::UnsupportedIndexShape {
                index_count: arguments.len(),
                range: expression.range,
                expression_id: expression.id,
            });
        }

        self.subset_result_type(value_type, value, expression, type_definitions)
    }

    fn subset_result_type(
        &mut self,
        value_type: CoreType,
        value: &Expression,
        expression: &Expression,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // Member-wise over a union subject: `[` must be valid on every shape the subject can take,
        // and the slice's type is the join of the per-member results. A failing member reports the
        // full union — the subject's actual type — not the single member that failed.
        if let CoreType::Union(members) = value_type {
            let union_type = CoreType::Union(members.clone());
            let mut results = Vec::with_capacity(members.len());
            for member in members {
                let member = self.resolve_structural(member, type_definitions, Some(value))?;
                let result = self
                    .subset_result_type(member, value, expression, type_definitions)
                    .map_err(|error| widen_error_container_to_union(error, &union_type))?;
                results.push(result);
            }
            return Ok(CoreType::union_of(results));
        }

        match value_type {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::List(item_type) => Ok(CoreType::List(item_type)),
            CoreType::NamedList(item_type) => Ok(CoreType::NamedList(item_type)),
            // A `[` slice of a fixed-shape list is a sub-list that can contain any of the item
            // types, so the element type is their union (collapsing back to the single item type
            // for a homogeneous list; slicing the empty list yields `list[NULL]`).
            CoreType::Tuple(items) => Ok(CoreType::List(Box::new(CoreType::union_of(items)))),
            CoreType::Record(fields) => Ok(CoreType::NamedList(Box::new(CoreType::union_of(
                fields.iter().map(|field| field.value.clone()).collect(),
            )))),
            // A sealed nominal has no modeled structure, but the R object behind it commonly
            // supports `[` with a value-dependent result (`df[rows]`, `f[levels]`): the slice is
            // `Unknown` — sound-by-refusal, surfaced under strict mode — not a hard error.
            CoreType::Nominal(..) => {
                self.record_strict_origin(
                    expression.id,
                    expression.range,
                    StrictOriginKind::UnsupportedConstruct,
                );
                Ok(CoreType::Unknown)
            }
            other_type => Err(InferenceError::UnsupportedSubset {
                actual: Box::new(other_type),
                range: expression.range,
                expression_id: expression.id,
            }),
        }
    }

    fn infer_subset2_expression(
        &mut self,
        value: &Expression,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let inferred_value =
            self.infer_expression_with_context(value, arena, resolution_context, type_definitions)?;
        let value_type = self.resolve_structural(inferred_value, type_definitions, Some(value))?;
        for argument in arguments {
            let argument_expression = arena.get(argument.expression);
            self.infer_expression_with_context(
                argument_expression,
                arena,
                resolution_context,
                type_definitions,
            )?;
        }

        if matches!(value_type, CoreType::Unknown) {
            return Ok(CoreType::Unknown);
        }
        if matches!(value_type, CoreType::Any) {
            return Ok(CoreType::Any);
        }
        // A sealed nominal: value-dependent element access, unmodeled — `Unknown` before the
        // index-arity check, exactly as for `[`.
        if matches!(value_type, CoreType::Nominal(..)) {
            self.record_strict_origin(
                expression.id,
                expression.range,
                StrictOriginKind::UnsupportedConstruct,
            );
            return Ok(CoreType::Unknown);
        }
        if arguments.len() != 1 || arguments[0].name.is_some() {
            return Err(InferenceError::UnsupportedIndexShape {
                index_count: arguments.len(),
                range: expression.range,
                expression_id: expression.id,
            });
        }
        let index_expression = arena.get(arguments[0].expression);

        self.subset2_result_type(
            value_type,
            value,
            index_expression,
            expression,
            type_definitions,
        )
    }

    fn subset2_result_type(
        &mut self,
        value_type: CoreType,
        value: &Expression,
        index_expression: &Expression,
        expression: &Expression,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // Member-wise over a union subject: `[[` must be valid on every shape the subject can
        // take, and the element's type is the join of the per-member results. A failing member
        // reports the full union — the subject's actual type — not the single member that failed.
        if let CoreType::Union(members) = value_type {
            let union_type = CoreType::Union(members.clone());
            let mut results = Vec::with_capacity(members.len());
            for member in members {
                let member = self.resolve_structural(member, type_definitions, Some(value))?;
                let result = self
                    .subset2_result_type(
                        member,
                        value,
                        index_expression,
                        expression,
                        type_definitions,
                    )
                    .map_err(|error| widen_error_container_to_union(error, &union_type))?;
                results.push(result);
            }
            return Ok(CoreType::union_of(results));
        }

        match value_type {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::Scalar(atomic) => Ok(CoreType::Scalar(atomic)),
            CoreType::Vector(element) => Ok(*element),
            CoreType::NamedVector(element) => {
                if literal_name_symbol(index_expression).is_some() {
                    Ok(nullable_type(*element))
                } else {
                    Ok(*element)
                }
            }
            CoreType::List(item_type) => Ok(*item_type),
            // Mirrors the map-like vector arm: a literal name may be absent at runtime (`T | NULL`),
            // while positional and computed access extract an item like R does on any list.
            CoreType::NamedList(item_type) => {
                if literal_name_symbol(index_expression).is_some() {
                    Ok(nullable_type(*item_type))
                } else {
                    Ok(*item_type)
                }
            }
            CoreType::Tuple(items) => {
                let Some(index) = integer_literal_position(index_expression) else {
                    return Err(InferenceError::NonLiteralSubscript {
                        container: Box::new(CoreType::Tuple(items)),
                        by: SubscriptKind::Position,
                        range: expression.range,
                        expression_id: expression.id,
                    });
                };
                match items.get(index).cloned() {
                    Some(item_type) => Ok(item_type),
                    None => Err(InferenceError::PositionDoesNotExist {
                        position: index + 1,
                        container: Box::new(CoreType::Tuple(items)),
                        range: expression.range,
                        expression_id: expression.id,
                    }),
                }
            }
            CoreType::Record(fields) => {
                // Record fields are declaration-ordered, so R's positional `[[` extracts the
                // field at that position exactly like a tuple item.
                if let Some(index) = integer_literal_position(index_expression) {
                    return match fields.get(index) {
                        Some(field) => Ok(field.value.clone()),
                        None => Err(InferenceError::PositionDoesNotExist {
                            position: index + 1,
                            container: Box::new(CoreType::Record(fields)),
                            range: expression.range,
                            expression_id: expression.id,
                        }),
                    };
                }
                let Some(name) = literal_name_symbol(index_expression) else {
                    return Err(InferenceError::NonLiteralSubscript {
                        container: Box::new(CoreType::Record(fields)),
                        by: SubscriptKind::FieldName,
                        range: expression.range,
                        expression_id: expression.id,
                    });
                };
                match fields.iter().find(|field| field.name == name) {
                    Some(field) => Ok(field.value.clone()),
                    None => Err(InferenceError::FieldDoesNotExist {
                        field: name,
                        container: Box::new(CoreType::Record(fields)),
                        range: expression.range,
                        expression_id: expression.id,
                    }),
                }
            }
            // A sealed nominal has no modeled structure, but the R object behind it commonly
            // supports `[[` with a value-dependent result (`df[["col"]]`): the element is
            // `Unknown` — sound-by-refusal, surfaced under strict mode — not a hard error.
            CoreType::Nominal(..) => {
                self.record_strict_origin(
                    expression.id,
                    expression.range,
                    StrictOriginKind::UnsupportedConstruct,
                );
                Ok(CoreType::Unknown)
            }
            other_type => Err(InferenceError::NotAList {
                actual: Box::new(other_type),
                range: expression.range,
                expression_id: expression.id,
            }),
        }
    }

    fn infer_dollar_expression(
        &mut self,
        value: &Expression,
        name: Symbol,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let inferred_value =
            self.infer_expression_with_context(value, arena, resolution_context, type_definitions)?;
        let value_type = self.resolve_structural(inferred_value, type_definitions, Some(value))?;

        self.dollar_result_type(value_type, value, name, expression, type_definitions)
    }

    fn dollar_result_type(
        &mut self,
        value_type: CoreType,
        value: &Expression,
        name: Symbol,
        expression: &Expression,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // Member-wise over a union subject: the field must exist on every shape the subject can
        // take, and its type is the join of the per-member results. A failing member reports the
        // full union — the subject's actual type — not the single member that failed.
        if let CoreType::Union(members) = value_type {
            let union_type = CoreType::Union(members.clone());
            let mut results = Vec::with_capacity(members.len());
            for member in members {
                let member = self.resolve_structural(member, type_definitions, Some(value))?;
                let result = self
                    .dollar_result_type(member, value, name, expression, type_definitions)
                    .map_err(|error| widen_error_container_to_union(error, &union_type))?;
                results.push(result);
            }
            return Ok(CoreType::union_of(results));
        }

        match value_type {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            // R rejects `$` on every atomic vector ("$ operator is invalid for atomic vectors"),
            // named ones included — element extraction is `[[`'s job.
            atomic @ (CoreType::Scalar(_) | CoreType::Vector(_) | CoreType::NamedVector(_)) => {
                Err(InferenceError::DollarOnAtomicVector {
                    actual: Box::new(atomic),
                    range: expression.range,
                    expression_id: expression.id,
                })
            }
            CoreType::NamedList(item_type) => Ok(nullable_type(*item_type)),
            CoreType::Record(fields) => match fields.iter().find(|field| field.name == name) {
                Some(field) => Ok(field.value.clone()),
                None => Err(InferenceError::FieldDoesNotExist {
                    field: name,
                    container: Box::new(CoreType::Record(fields)),
                    range: expression.range,
                    expression_id: expression.id,
                }),
            },
            container @ (CoreType::Tuple(_) | CoreType::List(_)) => {
                Err(InferenceError::FieldDoesNotExist {
                    field: name,
                    container: Box::new(container),
                    range: expression.range,
                    expression_id: expression.id,
                })
            }
            // A sealed nominal (`data.frame`, `factor`, ...) has no modeled structure, but the R
            // object behind it commonly supports `$` with a value-dependent result (`df$col`).
            // Refusing loudly here would error on the most idiomatic R there is, so the access is
            // `Unknown` — sound-by-refusal, surfaced under strict mode.
            CoreType::Nominal(..) => {
                self.record_strict_origin(
                    expression.id,
                    expression.range,
                    StrictOriginKind::UnsupportedConstruct,
                );
                Ok(CoreType::Unknown)
            }
            other_type => Err(InferenceError::NotAList {
                actual: Box::new(other_type),
                range: expression.range,
                expression_id: expression.id,
            }),
        }
    }

    fn infer_builtin_boolean_binary(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        if arguments.len() != 2 {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 2,
                actual: arguments.len(),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        self.expect_scalar_logical(
            arena.get(arguments[0].expression),
            arena,
            resolution_context,
            type_definitions,
        )?;
        self.expect_scalar_logical(
            arena.get(arguments[1].expression),
            arena,
            resolution_context,
            type_definitions,
        )?;
        Ok(CoreType::Scalar(Atomic::Logical))
    }

    fn infer_builtin_combine(
        &mut self,
        arguments: &[Argument],
        _expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        if arguments.is_empty() {
            return Ok(CoreType::Null);
        }

        let mut item_atomic = None;
        let mut all_arguments_are_named = true;
        let mut saw_non_null_argument = false;

        for argument in arguments {
            let arg_expr = arena.get(argument.expression);
            let inferred_argument = self.infer_expression_with_context(
                arg_expr,
                arena,
                resolution_context,
                type_definitions,
            )?;
            let resolved_argument =
                self.resolve_structural(inferred_argument, type_definitions, Some(arg_expr))?;

            // R drops `NULL` inside `c(...)`: `c(x, NULL)` is `c(x)` and `c(NULL)` is `NULL`.
            if resolved_argument == CoreType::Null {
                continue;
            }
            saw_non_null_argument = true;
            all_arguments_are_named &= argument.name.is_some();

            // A union argument combines member-wise. Its `NULL` members contribute nothing —
            // R drops `NULL` inside `c(...)`, so the idiomatic accumulator seeded with `c()`
            // (`acc <- c(); acc <- c(acc, x)` — type `T[] | NULL` at the loop join) is not an
            // error — and every remaining member must itself be combinable.
            let argument_atomics = match &resolved_argument {
                CoreType::Union(members) => members
                    .iter()
                    .filter(|member| !matches!(member, CoreType::Null))
                    .map(combine_operand_atomic)
                    .collect::<Option<Vec<Atomic>>>(),
                other => combine_operand_atomic(other).map(|atomic| vec![atomic]),
            };
            let Some(argument_atomics) = argument_atomics.filter(|atomics| !atomics.is_empty())
            else {
                return Err(InferenceError::TypeMismatch {
                    expected: Box::new(CoreType::Scalar(Atomic::Integer)),
                    actual: Box::new(resolved_argument.clone()),
                    range: Some(arg_expr.range),
                    expression_id: Some(arg_expr.id),
                });
            };

            for current_atomic in argument_atomics {
                item_atomic = Some(match item_atomic {
                    Some(previous_atomic) => {
                        promote_combine_atomic(previous_atomic, current_atomic).ok_or_else(
                            || InferenceError::TypeMismatch {
                                expected: Box::new(CoreType::Scalar(previous_atomic)),
                                actual: Box::new(resolved_argument.clone()),
                                range: Some(arg_expr.range),
                                expression_id: Some(arg_expr.id),
                            },
                        )?
                    }
                    None => current_atomic,
                });
            }
        }

        if !saw_non_null_argument {
            return Ok(CoreType::Null);
        }
        let combined_atomic = item_atomic.unwrap_or(Atomic::Integer);
        if all_arguments_are_named {
            Ok(CoreType::named_vector(combined_atomic))
        } else {
            Ok(CoreType::vector(combined_atomic))
        }
    }

    // `switch(subject, a = ..., b = ..., default)` selects one branch by the subject's runtime
    // value. Selection cannot be modeled statically, but every branch IS checked — errors inside a
    // branch surface like anywhere else — and the call's type is the union of the branch values.
    // R returns invisible `NULL` when nothing matches, so `NULL` joins the union unless a default
    // (unnamed, non-first) branch exists. A named branch with no value falls through to the next
    // branch in R; it contributes no type of its own.
    fn infer_builtin_switch(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let Some((subject, branches)) = arguments.split_first() else {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 1,
                actual: 0,
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        };
        self.infer_expression_with_context(
            arena.get(subject.expression),
            arena,
            resolution_context,
            type_definitions,
        )?;

        let mut members = Vec::with_capacity(branches.len() + 1);
        let mut has_default = false;
        for branch in branches {
            if branch.name.is_none() {
                has_default = true;
            }
            let branch_type = self.infer_expression_with_context(
                arena.get(branch.expression),
                arena,
                resolution_context,
                type_definitions,
            )?;
            members.push(self.resolve(branch_type)?);
        }
        if !has_default {
            members.push(CoreType::Null);
        }
        Ok(CoreType::union_of(members))
    }

    fn infer_builtin_list(
        &mut self,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        if arguments.is_empty() {
            return Ok(CoreType::Tuple(Vec::new()));
        }

        let all_arguments_are_named = arguments.iter().all(|argument| argument.name.is_some());
        let all_arguments_are_unnamed = arguments.iter().all(|argument| argument.name.is_none());

        if !(all_arguments_are_named || all_arguments_are_unnamed) {
            return Err(InferenceError::MixedListElements {
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        if all_arguments_are_named {
            let mut fields = Vec::with_capacity(arguments.len());
            for argument in arguments {
                let inferred_type = self.infer_expression_with_context(
                    arena.get(argument.expression),
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
                let inferred_type = self.resolve(inferred_type)?;
                fields.push(RecordField::new(
                    argument
                        .name
                        .expect("named list arguments should have names"),
                    inferred_type,
                ));
            }
            Ok(CoreType::Record(fields))
        } else {
            let mut items = Vec::with_capacity(arguments.len());
            for argument in arguments {
                let inferred_type = self.infer_expression_with_context(
                    arena.get(argument.expression),
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
                items.push(self.resolve(inferred_type)?);
            }
            Ok(CoreType::Tuple(items))
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{interner::Interner, types::Atomic},
    };

    // Exercises the private rigid-variable allocation path, which the integration tests cannot reach.
    // Without recording the `rigid_variables` insert, a rollback would reclaim the id but leave it
    // marked rigid, so the next `fresh_variable()` would wrongly refuse to bind.
    #[test]
    fn rollback_reclaims_a_rigid_id_so_it_is_no_longer_rigid() {
        let mut interner = Interner::new();
        let name = interner.intern("T");
        let mut inference_state = InferenceState::new();

        let snapshot = inference_state.snapshot();
        let rigid = inference_state.fresh_rigid_variable(name, Constraint::Unconstrained);
        assert!(
            inference_state
                .unify(CoreType::Variable(rigid), CoreType::Scalar(Atomic::Integer))
                .is_err(),
            "a rigid skolem must not bind to a concrete type"
        );

        inference_state.rollback_to(snapshot);

        let reused = inference_state.fresh_variable();
        assert_eq!(reused, rigid, "the rigid id should be reclaimed");
        inference_state
            .unify(
                CoreType::Variable(reused),
                CoreType::Scalar(Atomic::Integer),
            )
            .expect("the reclaimed id must no longer be treated as rigid");
    }

    // `check_compatibility` is private, so the S1 acceptance evidence lives in-crate rather than in
    // the external `tests/test_typecheck.rs`.
    fn builtins_state() -> InferenceState {
        let mut interner = Interner::new();
        inference_state_with_builtins_in_interner(&mut interner)
    }

    fn tuple(items: Vec<CoreType>) -> CoreType {
        CoreType::Tuple(items)
    }

    fn check(state: &mut InferenceState, actual: CoreType, expected: CoreType) -> bool {
        state
            .check_compatibility(
                actual,
                expected,
                &TypeDefinitionEnvironment::default(),
                None,
            )
            .expect("structural compatibility check should not error")
    }

    // The boolean outcome of a check must not depend on the order it runs in: because a failed check
    // reverses its speculative bindings, a check on a structural pair gives the same result whichever
    // side is `actual` (each side's variable binds against the other on the way to the verdict).
    #[test]
    fn check_compatibility_outcome_is_order_independent() {
        // Incompatible pair: element 0 binds a variable, element 1 fails -> false either direction.
        let mut forward = builtins_state();
        let a = forward.fresh_variable();
        let forward_incompatible = check(
            &mut forward,
            tuple(vec![
                CoreType::Variable(a),
                CoreType::Scalar(Atomic::Logical),
            ]),
            tuple(vec![
                CoreType::Scalar(Atomic::Integer),
                CoreType::Scalar(Atomic::Integer),
            ]),
        );
        let mut backward = builtins_state();
        let b = backward.fresh_variable();
        let backward_incompatible = check(
            &mut backward,
            tuple(vec![
                CoreType::Scalar(Atomic::Integer),
                CoreType::Scalar(Atomic::Integer),
            ]),
            tuple(vec![
                CoreType::Variable(b),
                CoreType::Scalar(Atomic::Logical),
            ]),
        );
        assert!(!forward_incompatible);
        assert_eq!(forward_incompatible, backward_incompatible);

        // Compatible pair: both elements succeed -> true either direction.
        let mut forward = builtins_state();
        let a = forward.fresh_variable();
        let forward_compatible = check(
            &mut forward,
            tuple(vec![
                CoreType::Variable(a),
                CoreType::Scalar(Atomic::Integer),
            ]),
            tuple(vec![
                CoreType::Scalar(Atomic::Integer),
                CoreType::Scalar(Atomic::Integer),
            ]),
        );
        let mut backward = builtins_state();
        let b = backward.fresh_variable();
        let backward_compatible = check(
            &mut backward,
            tuple(vec![
                CoreType::Scalar(Atomic::Integer),
                CoreType::Scalar(Atomic::Integer),
            ]),
            tuple(vec![
                CoreType::Variable(b),
                CoreType::Scalar(Atomic::Integer),
            ]),
        );
        assert!(forward_compatible);
        assert_eq!(forward_compatible, backward_compatible);
    }

    // A check that returns `false` after binding an inner field must leave ZERO net mutation: under
    // the pre-purity code, element 0 below bound `a` to `integer` before element 1 failed, leaking a
    // partial binding. The wrapper now rolls that back.
    #[test]
    fn failing_check_leaves_no_partial_binding() {
        let mut state = builtins_state();
        let a = state.fresh_variable();
        let b = state.fresh_variable();
        let entry_count_before = state.entries.len();
        let next_id_before = state.next_variable_id;

        let result = check(
            &mut state,
            tuple(vec![
                CoreType::Variable(a),
                CoreType::Scalar(Atomic::Logical),
            ]),
            tuple(vec![
                CoreType::Scalar(Atomic::Integer),
                CoreType::Scalar(Atomic::Integer),
            ]),
        );

        assert!(!result, "logical is not compatible with integer");
        assert_eq!(
            state.entry(a),
            Some(&unbound()),
            "the partial binding of `a` must be reversed"
        );
        assert_eq!(state.entry(b), Some(&unbound()));
        assert_eq!(state.entries.len(), entry_count_before, "no leaked entries");
        assert_eq!(
            state.next_variable_id, next_id_before,
            "no leaked variable ids"
        );
    }

    // The complement of purity: a SUCCESSFUL check keeps the bindings it makes, which is how `@new`
    // and checked annotations infer their type arguments.
    #[test]
    fn successful_check_keeps_its_bindings() {
        let mut state = builtins_state();
        let a = state.fresh_variable();

        let result = check(
            &mut state,
            tuple(vec![
                CoreType::Variable(a),
                CoreType::Scalar(Atomic::Integer),
            ]),
            tuple(vec![
                CoreType::Scalar(Atomic::Integer),
                CoreType::Scalar(Atomic::Integer),
            ]),
        );

        assert!(result);
        assert_eq!(
            state.entry(a),
            Some(&InferenceEntry::Bound(CoreType::Scalar(Atomic::Integer))),
            "a successful check must keep its inferred binding"
        );
    }

    fn unbound() -> InferenceEntry {
        InferenceEntry::Unbound {
            level: 0,
            constraint: Constraint::Unconstrained,
        }
    }
}
