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
            InferenceVariableId, QuantifiedVariable, RecordField, SurfaceType, TypeAnnotationKind,
            TypeScheme,
        },
    },
    std::collections::{BTreeMap, BTreeSet},
    tree_sitter::Range,
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

// A type-guard predicate recognized in `if` conditions (`is.null(x)`, `is.character(x)`, ...).
// Guards narrow the guarded local variable's type along the branch edges; see the guard-narrowing
// section of the typing reference for the exact filtering rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuardPredicate {
    Null,
    Character,
    Logical,
    Integer,
    Double,
    Numeric,
    Function,
    List,
}

// The names each guard predicate answers to. Seeded into `guard_predicates` by the builtin
// constructors so condition inspection is a symbol lookup, not a string compare.
const GUARD_PREDICATES: &[(&str, GuardPredicate)] = &[
    ("is.null", GuardPredicate::Null),
    ("is.character", GuardPredicate::Character),
    ("is.logical", GuardPredicate::Logical),
    ("is.integer", GuardPredicate::Integer),
    ("is.double", GuardPredicate::Double),
    ("is.numeric", GuardPredicate::Numeric),
    ("is.function", GuardPredicate::Function),
    ("is.list", GuardPredicate::List),
];

// A guard's effect on the guarded slot: the entry to install on each edge (`None` = that edge
// leaves the entry untouched). `range` preserves the original binding's definition range.
struct GuardRefinement {
    key: EnvironmentKey,
    range: Range,
    true_type: Option<CoreType>,
    false_type: Option<CoreType>,
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
            if let Some(element) = &function_type.variadic {
                accumulate_parameter_variances(element, polarity.flip(), parameters, variances);
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

    pub fn fresh_variable(&mut self) -> InferenceVariableId {
        self.fresh_constrained_variable(Constraint::Unconstrained)
    }

    fn fresh_rigid_variable(
        &mut self,
        name: Symbol,
        constraint: Constraint,
    ) -> InferenceVariableId {
        let variable = self.fresh_constrained_variable(constraint);
        self.set_rigid(variable, name);
        variable
    }

    // Renders a rigid variable as its declared type-parameter name (e.g. `T`) for diagnostics, so a
    // failed polymorphic-annotation check reads `expected T, found integer` instead of `type1`.
    fn rigid_display(&self, variable: InferenceVariableId) -> CoreType {
        match self.rigid_variables.get(&variable) {
            Some(name) => CoreType::Nominal(*name, Vec::new()),
            None => CoreType::Variable(variable),
        }
    }

    // Resolves `core_type` and replaces every rigid skolem variable with its declared name, so a
    // diagnostic involving a `<T>` annotation shows `T` rather than an internal `type1`.
    fn display_with_rigid_names(&mut self, core_type: &CoreType) -> CoreType {
        // Pure display path: a resolve failure degrades to `Unknown` rather than propagating, so
        // rendering a diagnostic or hover never aborts the check that produced it.
        let resolved = self.resolve(core_type.clone()).unwrap_or(CoreType::Unknown);
        self.substitute_rigid_names(&resolved)
    }

    fn substitute_rigid_names(&self, core_type: &CoreType) -> CoreType {
        match core_type {
            CoreType::Variable(variable) => self.rigid_display(*variable),
            CoreType::Union(members) => CoreType::union_of(
                members
                    .iter()
                    .map(|member| self.substitute_rigid_names(member))
                    .collect(),
            ),
            CoreType::Vector(element) => {
                CoreType::Vector(Box::new(self.substitute_rigid_names(element)))
            }
            CoreType::NamedVector(element) => {
                CoreType::NamedVector(Box::new(self.substitute_rigid_names(element)))
            }
            CoreType::List(inner) => CoreType::List(Box::new(self.substitute_rigid_names(inner))),
            CoreType::NamedList(inner) => {
                CoreType::NamedList(Box::new(self.substitute_rigid_names(inner)))
            }
            CoreType::Tuple(items) => CoreType::Tuple(
                items
                    .iter()
                    .map(|item| self.substitute_rigid_names(item))
                    .collect(),
            ),
            CoreType::Record(fields) => CoreType::Record(
                fields
                    .iter()
                    .map(|field| {
                        RecordField::with_optional(
                            field.name,
                            self.substitute_rigid_names(&field.value),
                            field.optional,
                        )
                    })
                    .collect(),
            ),
            CoreType::Nominal(name, arguments) => CoreType::Nominal(
                *name,
                arguments
                    .iter()
                    .map(|argument| self.substitute_rigid_names(argument))
                    .collect(),
            ),
            CoreType::Function(function_type) => CoreType::Function(FunctionType::with_variadic(
                function_type
                    .parameters
                    .iter()
                    .map(|parameter| self.substitute_rigid_names(parameter))
                    .collect(),
                function_type
                    .named_parameters
                    .iter()
                    .map(|parameter| {
                        RecordField::with_optional(
                            parameter.name,
                            self.substitute_rigid_names(&parameter.value),
                            parameter.optional,
                        )
                    })
                    .collect(),
                function_type
                    .variadic
                    .as_ref()
                    .map(|element| self.substitute_rigid_names(element)),
                self.substitute_rigid_names(&function_type.return_type),
            )),
            other => other.clone(),
        }
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

    // Begins a speculative region for probing (e.g. a trial unification) that can be discarded.
    //
    // Probe contract — what a snapshot does and does NOT reverse:
    // - REVERSED: every union-find write (`entries`, via `set_entry`), every rigid-variable marker
    //   (`rigid_variables`, via `set_rigid`), and `next_variable_id` (so ids allocated inside the
    //   probe are reclaimed). These are exactly the fields the resolve / unify / check_compatibility
    //   / representation- and alias-lowering paths touch, which is all a `check_compatibility` probe
    //   can reach.
    // - NOT reversed: `environment`, `recorded_expression_types`, and `current_level`. This is safe
    //   for the intended probe use: `environment` and `recorded_expression_types` are mutated only by
    //   binding inference and expression-type recording, never by the compatibility/unification paths
    //   a probe runs; and `current_level` is balanced by paired `enter_level`/`exit_level`, so a probe
    //   that does not leak an unbalanced level change leaves it untouched. `recursion_depth` is
    //   likewise transient and deliberately excluded. A probe must keep its writes within this contract.
    //
    // Nested snapshots compose: an inner rollback truncates the log to the inner mark, leaving outer
    // writes intact.
    pub fn snapshot(&mut self) -> Snapshot {
        self.snapshot_depth += 1;
        Snapshot {
            log_len: self.undo_log.len(),
            next_variable_id: self.next_variable_id,
        }
    }

    // Reverses every recorded write made since `snapshot` (see the probe contract on `snapshot`),
    // restoring entries and rigid markers and reclaiming the variable ids allocated in between.
    // Leaves `recursion_depth` and the non-reversed fields untouched.
    pub fn rollback_to(&mut self, snapshot: Snapshot) {
        debug_assert!(
            self.snapshot_depth > 0,
            "rollback_to without an open snapshot"
        );
        while self.undo_log.len() > snapshot.log_len {
            match self.undo_log.pop() {
                Some(UndoStep::Entry { variable, previous }) => match previous {
                    Some(entry) => {
                        self.entries.insert(variable, entry);
                    }
                    None => {
                        self.entries.remove(&variable);
                    }
                },
                Some(UndoStep::Rigid { variable, previous }) => match previous {
                    Some(name) => {
                        self.rigid_variables.insert(variable, name);
                    }
                    None => {
                        self.rigid_variables.remove(&variable);
                    }
                },
                None => break,
            }
        }

        // Variable ids allocated after the snapshot are reclaimed. The log already removes their
        // `entries` and `rigid_variables` records; dropping any survivor with an id at or above the
        // restored counter is a safety net, since all ids are allocated monotonically and so cannot
        // predate the snapshot (and therefore cannot collide with a pre-existing variable or rigid).
        let stale_variables = self
            .entries
            .range(InferenceVariableId(snapshot.next_variable_id)..)
            .map(|(variable, _)| *variable)
            .collect::<Vec<_>>();
        for variable in stale_variables {
            self.entries.remove(&variable);
        }
        let stale_rigids = self
            .rigid_variables
            .range(InferenceVariableId(snapshot.next_variable_id)..)
            .map(|(variable, _)| *variable)
            .collect::<Vec<_>>();
        for variable in stale_rigids {
            self.rigid_variables.remove(&variable);
        }
        self.next_variable_id = snapshot.next_variable_id;

        self.snapshot_depth -= 1;
    }

    // Keeps every write made since `snapshot`. The log is retained while an outer snapshot is still
    // active (it may yet roll back over these writes) and cleared once the outermost region commits.
    pub fn commit(&mut self, snapshot: Snapshot) {
        debug_assert!(self.snapshot_depth > 0, "commit without an open snapshot");
        debug_assert!(self.undo_log.len() >= snapshot.log_len);
        self.snapshot_depth -= 1;
        if self.snapshot_depth == 0 {
            self.undo_log.clear();
        }
    }

    // The single chokepoint for union-find writes: records the prior entry (when a snapshot is
    // active) before overwriting, so the undo log stays complete and the committed path stays free.
    fn set_entry(&mut self, variable: InferenceVariableId, entry: InferenceEntry) {
        if self.snapshot_depth > 0 {
            let previous = self.entries.get(&variable).cloned();
            self.undo_log.push(UndoStep::Entry { variable, previous });
        }
        self.entries.insert(variable, entry);
    }

    // The chokepoint for rigid-variable markers, mirroring `set_entry` so a probe can reverse a
    // skolem allocation that would otherwise leave a reclaimed id wrongly marked rigid.
    fn set_rigid(&mut self, variable: InferenceVariableId, name: Symbol) {
        if self.snapshot_depth > 0 {
            let previous = self.rigid_variables.get(&variable).copied();
            self.undo_log.push(UndoStep::Rigid { variable, previous });
        }
        self.rigid_variables.insert(variable, name);
    }

    fn fresh_constrained_variable(&mut self, constraint: Constraint) -> InferenceVariableId {
        let variable = InferenceVariableId(self.next_variable_id);
        self.next_variable_id += 1;
        self.set_entry(
            variable,
            InferenceEntry::Unbound {
                level: self.current_level,
                constraint,
            },
        );
        variable
    }

    // Raises the bound on whatever `core_type` resolves to. When it resolves to an unbound
    // variable, the variable records the stronger constraint; when it resolves to a concrete
    // type, the type itself must already satisfy the constraint.
    fn constrain_type(
        &mut self,
        core_type: CoreType,
        constraint: Constraint,
        expression: Option<&Expression>,
    ) -> Result<(), InferenceError> {
        if constraint == Constraint::Unconstrained {
            return Ok(());
        }
        match self.resolve(core_type)? {
            CoreType::Variable(variable) => {
                // A rigid skolem stands for every possible T; it cannot carry a constraint (e.g. a
                // `<T>` body that does `value + 1L` would require T to be numeric, which the declared
                // unconstrained `<T>` does not promise).
                if self.rigid_variables.contains_key(&variable) {
                    return Err(InferenceError::ConstraintViolation {
                        constraint,
                        actual: Box::new(self.rigid_display(variable)),
                        range: expression.map(|current| current.range),
                        expression_id: expression.map(|current| current.id),
                    });
                }
                let raised_entry = match self.entries.get(&variable) {
                    Some(InferenceEntry::Unbound {
                        level,
                        constraint: existing,
                    }) => Some(InferenceEntry::Unbound {
                        level: *level,
                        constraint: (*existing).join(constraint),
                    }),
                    _ => None,
                };
                if let Some(entry) = raised_entry {
                    self.set_entry(variable, entry);
                }
                Ok(())
            }
            concrete_type if constraint_is_satisfied(constraint, &concrete_type) => Ok(()),
            concrete_type => Err(constraint_violation_error(
                constraint,
                concrete_type,
                expression,
            )),
        }
    }

    fn enter_level(&mut self) {
        self.current_level += 1;
    }

    fn exit_level(&mut self) {
        self.current_level -= 1;
    }

    pub fn entry(&self, variable: InferenceVariableId) -> Option<&InferenceEntry> {
        self.entries.get(&variable)
    }

    pub fn bind_global_name(&mut self, symbol: Symbol, core_type: CoreType, range: Range) {
        self.bind_global_scheme(symbol, TypeScheme::monomorphic(core_type), range);
    }

    pub fn bind_name(&mut self, symbol: Symbol, core_type: CoreType, range: Range) {
        self.bind_global_name(symbol, core_type, range);
    }

    pub fn bind_global_scheme(&mut self, symbol: Symbol, type_scheme: TypeScheme, range: Range) {
        self.set_environment_entry(
            EnvironmentKey::Global(symbol),
            Some(Binding { type_scheme, range }),
        );
    }

    pub fn bind_scheme(&mut self, symbol: Symbol, type_scheme: TypeScheme, range: Range) {
        self.bind_global_scheme(symbol, type_scheme, range);
    }

    fn bind_local_name(&mut self, binding_id: BindingId, core_type: CoreType, range: Range) {
        self.bind_local_scheme(binding_id, TypeScheme::monomorphic(core_type), range);
    }

    fn bind_local_scheme(&mut self, binding_id: BindingId, type_scheme: TypeScheme, range: Range) {
        self.set_environment_entry(
            EnvironmentKey::Local(binding_id),
            Some(Binding { type_scheme, range }),
        );
    }

    // The single chokepoint for environment writes: records the key's prior value while an
    // environment snapshot is active so a control-flow region can be reverted.
    fn set_environment_entry(&mut self, key: EnvironmentKey, entry: Option<Binding>) {
        self.log_loop_access(key);
        let previous = match entry {
            Some(binding) => self.environment.insert(key, binding),
            None => self.environment.remove(&key),
        };
        if self.environment_snapshot_depth > 0 {
            self.environment_log.push((key, previous));
        }
    }

    // Records `key` (with its current value on first touch) in every active loop region's access
    // log; the logs become the regions' memo guards.
    fn log_loop_access(&mut self, key: EnvironmentKey) {
        if self.loop_access_logs.is_empty() {
            return;
        }
        let value = self.environment.get(&key).cloned();
        for log in &mut self.loop_access_logs {
            if !log.accesses.contains_key(&key) {
                if !log.first_pass {
                    log.complete = false;
                }
                log.accesses.insert(key, value.clone());
            }
        }
    }

    // Begins an environment region (a branch, a loop pass, or a function body) whose writes will
    // be reverted by `environment_rollback`. Nested regions compose like unification snapshots.
    fn environment_snapshot(&mut self) -> EnvironmentSnapshot {
        self.environment_snapshot_depth += 1;
        EnvironmentSnapshot {
            log_length: self.environment_log.len(),
        }
    }

    // Reverts every environment write recorded since `snapshot` and returns each touched key's
    // value at the moment of rollback — the region's final values (`None` = the region removed the
    // entry) — so a control-flow join can merge them with the restored pre-state.
    // Closes an environment region *keeping* its writes: the recorded entries stay in the log so
    // they revert with the enclosing region instead (at depth zero there is nothing to revert
    // into, so the log clears). Used when a discovery pass turns out to be the only pass needed.
    fn environment_commit(&mut self, _snapshot: EnvironmentSnapshot) {
        debug_assert!(
            self.environment_snapshot_depth > 0,
            "environment commit without an open snapshot"
        );
        self.environment_snapshot_depth -= 1;
        if self.environment_snapshot_depth == 0 {
            self.environment_log.clear();
        }
    }

    fn environment_rollback(
        &mut self,
        snapshot: EnvironmentSnapshot,
    ) -> BTreeMap<EnvironmentKey, Option<Binding>> {
        debug_assert!(
            self.environment_snapshot_depth > 0,
            "environment rollback without an open snapshot"
        );
        self.environment_snapshot_depth -= 1;
        let recorded = self.environment_log.split_off(snapshot.log_length);
        let mut region_values = BTreeMap::new();
        for (key, _) in &recorded {
            region_values
                .entry(*key)
                .or_insert_with(|| self.environment.get(key).cloned());
        }
        for (key, previous) in recorded.into_iter().rev() {
            match previous {
                Some(binding) => {
                    self.environment.insert(key, binding);
                }
                None => {
                    self.environment.remove(&key);
                }
            }
        }
        region_values
    }

    // Merges the environment effects of two alternative control-flow paths (both already rolled
    // back, so the environment currently holds the shared pre-state): every key either path
    // touched gets the join of its two path-final values, with an untouched path contributing the
    // pre-state value.
    fn join_branch_environments(
        &mut self,
        mut left: BTreeMap<EnvironmentKey, Option<Binding>>,
        mut right: BTreeMap<EnvironmentKey, Option<Binding>>,
        expression: &Expression,
    ) -> Result<(), InferenceError> {
        let keys: BTreeSet<EnvironmentKey> = left.keys().chain(right.keys()).copied().collect();
        for key in keys {
            let pre_state = self.environment.get(&key).cloned();
            let left_value = left.remove(&key).unwrap_or_else(|| pre_state.clone());
            let right_value = right.remove(&key).unwrap_or(pre_state);
            let joined = self.join_environment_entries(left_value, right_value, expression)?;
            self.set_environment_entry(key, joined);
        }
        Ok(())
    }

    // The binding a variable slot holds after two control paths merge. Identical entries stay; a
    // slot written on only one path optimistically keeps the written binding (a read on the
    // unwritten path is covered by the naming-level maybe-undefined warning); genuinely different
    // entries join into a monotype via `join_types` — a polymorphic scheme survives only while a
    // single write reaches (the generalization rule in the control-flow-joins section of the
    // typing reference).
    fn join_environment_entries(
        &mut self,
        left: Option<Binding>,
        right: Option<Binding>,
        expression: &Expression,
    ) -> Result<Option<Binding>, InferenceError> {
        match (left, right) {
            (None, None) => Ok(None),
            (Some(binding), None) | (None, Some(binding)) => Ok(Some(binding)),
            (Some(left), Some(right)) => {
                if left == right {
                    return Ok(Some(left));
                }
                let range = left.range;
                let left_type = self.instantiate_type_scheme(&left.type_scheme)?;
                let right_type = self.instantiate_type_scheme(&right.type_scheme)?;
                let left_type = self.resolve(left_type)?;
                let right_type = self.resolve(right_type)?;
                // An unmodelled path makes the merged slot unmodelled, matching how `Unknown`
                // absorbs unions everywhere else in the checker.
                let joined = if left_type == CoreType::Unknown || right_type == CoreType::Unknown {
                    CoreType::Unknown
                } else {
                    let joined = self.join_types(left_type, right_type, expression)?;
                    self.resolve(joined)?
                };
                Ok(Some(Binding {
                    type_scheme: TypeScheme::monomorphic(joined),
                    range,
                }))
            }
        }
    }

    pub fn bind_builtin(&mut self, symbol: Symbol, builtin_kind: BuiltinKind) {
        self.builtins.insert(symbol, builtin_kind);
    }

    fn lookup_local_name(&self, binding_id: BindingId) -> Option<&Binding> {
        self.environment.get(&EnvironmentKey::Local(binding_id))
    }

    pub fn bind_overload_set(&mut self, symbol: Symbol, schemes: Vec<TypeScheme>) {
        self.overload_sets.insert(symbol, schemes);
    }

    pub fn lookup_global_name(&self, symbol: Symbol) -> Option<&Binding> {
        self.environment.get(&EnvironmentKey::Global(symbol))
    }

    pub fn lookup_name(&self, symbol: Symbol) -> Option<&Binding> {
        self.lookup_global_name(symbol)
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
            ExpressionKind::Function { parameters, body } => self.infer_function_expression(
                expression.id,
                parameters,
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
            type_scheme: TypeScheme::monomorphic(written),
            range: expression.range,
        };
        let joined = self.join_environment_entries(current, Some(written_binding), expression)?;
        self.set_environment_entry(key, joined);
        Ok(())
    }

    // Bookkeeping for a write to a captured slot that has post-capture writes: accumulates the
    // slot's write join (variables erased so the entry survives unification rollbacks) and flags
    // the discovery re-pass.
    fn note_slot_write(
        &mut self,
        local_naming: &NamesLocal,
        binding_id: BindingId,
        written: &CoreType,
        expression: &Expression,
    ) -> Result<(), InferenceError> {
        if !local_naming.capture_repass_slots.contains(&binding_id) {
            return Ok(());
        }
        self.wrote_repass_slot = true;
        let sanitized = self.erase_inference_variables(written.clone())?;
        let joined = match self.captured_write_joins.get(&binding_id).cloned() {
            None => sanitized,
            Some(existing) => {
                if existing == CoreType::Unknown || sanitized == CoreType::Unknown {
                    CoreType::Unknown
                } else {
                    let joined = self.join_types(existing, sanitized, expression)?;
                    self.erase_inference_variables(joined)?
                }
            }
        };
        self.captured_write_joins.insert(binding_id, joined);
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
        if let ExpressionKind::Function { parameters, body } = &value_expression.kind
            && let Some(expected_function_type) =
                self.checked_function_annotation(annotation, type_definitions, expression)?
        {
            return self.infer_function_expression(
                value_expression.id,
                parameters,
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
        self.return_type_frames.push(Vec::new());
        let body_result = self.infer_body_with_capture_discovery(
            body,
            arena,
            resolution_context,
            type_definitions,
        );
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
        // sites.
        let named_parameter_types = parameters
            .iter()
            .zip(parameter_types)
            .map(|(parameter, parameter_type)| {
                RecordField::with_optional(
                    parameter.symbol,
                    parameter_type,
                    parameter.has_default(),
                )
            })
            .collect();
        Ok(FunctionType::new(
            Vec::new(),
            named_parameter_types,
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

    #[allow(clippy::too_many_arguments)]
    fn infer_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Expression,
        alternative: Option<&Expression>,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        self.expect_scalar_logical(condition, arena, resolution_context, type_definitions)?;

        // A type-guard condition refines the guarded slot along the branch edges. The refinement
        // is an ordinary undo-logged entry write inside the branch region, so a branch write
        // simply replaces it and the join below sees final values either way.
        let refinement = self.condition_refinement(condition, arena, resolution_context)?;

        let snapshot = self.environment_snapshot();
        if let Some(refinement) = &refinement
            && let Some(true_type) = &refinement.true_type
        {
            self.set_environment_entry(
                refinement.key,
                Some(Binding {
                    type_scheme: TypeScheme::monomorphic(true_type.clone()),
                    range: refinement.range,
                }),
            );
        }
        let consequence_result = self.infer_expression_with_context(
            consequence,
            arena,
            resolution_context,
            type_definitions,
        );
        let consequence_bindings = self.environment_rollback(snapshot);
        let consequence_type = self.resolve(consequence_result?)?;
        let consequence_diverges = self.expression_diverges(arena, consequence.id);

        // The false edge: the `else` branch when present, otherwise a synthetic empty region that
        // carries just the false-edge refinement — that is what survives past the `if` when the
        // consequence diverges (the early-exit guard pattern).
        let mut alternative_outcome = None;
        let alternative_bindings = match alternative {
            Some(alternative) => {
                let snapshot = self.environment_snapshot();
                if let Some(refinement) = &refinement
                    && let Some(false_type) = &refinement.false_type
                {
                    self.set_environment_entry(
                        refinement.key,
                        Some(Binding {
                            type_scheme: TypeScheme::monomorphic(false_type.clone()),
                            range: refinement.range,
                        }),
                    );
                }
                let alternative_result = self.infer_expression_with_context(
                    alternative,
                    arena,
                    resolution_context,
                    type_definitions,
                );
                let bindings = self.environment_rollback(snapshot);
                alternative_outcome = Some((
                    self.resolve(alternative_result?)?,
                    self.expression_diverges(arena, alternative.id),
                ));
                bindings
            }
            None => {
                let mut bindings = BTreeMap::new();
                if let Some(refinement) = &refinement
                    && let Some(false_type) = &refinement.false_type
                {
                    bindings.insert(
                        refinement.key,
                        Some(Binding {
                            type_scheme: TypeScheme::monomorphic(false_type.clone()),
                            range: refinement.range,
                        }),
                    );
                }
                bindings
            }
        };

        // A diverging branch never falls through, so it contributes no state: the surviving edge's
        // final values (refinement included) apply directly instead of joining with a path that
        // cannot reach the code after the `if`. When neither (or both — dead code) diverge, every
        // touched slot joins as before: pre-state first, so a union reads in execution order.
        let alternative_diverges = alternative_outcome
            .as_ref()
            .is_some_and(|(_, diverges)| *diverges);
        match (consequence_diverges, alternative_diverges) {
            (true, false) => {
                for (key, value) in alternative_bindings {
                    self.set_environment_entry(key, value);
                }
            }
            (false, true) => {
                for (key, value) in consequence_bindings {
                    self.set_environment_entry(key, value);
                }
            }
            _ => {
                if alternative.is_some() {
                    self.join_branch_environments(
                        consequence_bindings,
                        alternative_bindings,
                        expression,
                    )?;
                } else {
                    // Fall-through side first, so a union reads in execution order (the pre-state
                    // before the branch's retype: `integer | character`, not the reverse).
                    self.join_branch_environments(
                        alternative_bindings,
                        consequence_bindings,
                        expression,
                    )?;
                }
            }
        }

        let Some((alternative_type, _)) = alternative_outcome else {
            // Without an `else` the construct may fall through untouched, contributing `NULL`; a
            // diverging branch never yields at all, so the whole expression is plain `NULL`.
            return Ok(if consequence_diverges {
                CoreType::Null
            } else {
                nullable_type(consequence_type)
            });
        };

        // A diverging branch contributes no value either: `x <- if (c) return(NULL) else 5`
        // gives `x` the surviving branch's type.
        if consequence_diverges != alternative_diverges {
            return Ok(if consequence_diverges {
                alternative_type
            } else {
                consequence_type
            });
        }

        // An unmodelled branch makes the result unmodelled rather than claiming the other branch's
        // type, matching how `Unknown` propagates (and absorbs unions) through the rest of the
        // checker.
        if consequence_type == CoreType::Unknown || alternative_type == CoreType::Unknown {
            return Ok(CoreType::Unknown);
        }

        self.join_types(consequence_type, alternative_type, expression)
    }

    // The guard refinement an `if` condition induces, when the condition is a recognized predicate
    // applied to a plain local variable read (negation swaps the edges). `None` when the condition
    // is no guard, the callee is locally shadowed, the argument is not a resolved local slot, or
    // the guard cannot change the entry's type (see the guard-narrowing section of the typing
    // reference for the filtering rules).
    fn condition_refinement(
        &mut self,
        condition: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
    ) -> Result<Option<GuardRefinement>, InferenceError> {
        match &condition.kind {
            ExpressionKind::UnaryNot { value } => Ok(self
                .condition_refinement(arena.get(*value), arena, resolution_context)?
                .map(|refinement| GuardRefinement {
                    key: refinement.key,
                    range: refinement.range,
                    true_type: refinement.false_type,
                    false_type: refinement.true_type,
                })),
            ExpressionKind::Call { callee, arguments } => {
                let callee_expression = arena.get(*callee);
                let ExpressionKind::Symbol(callee_symbol) = &callee_expression.kind else {
                    return Ok(None);
                };
                let Some(predicate) = self.guard_predicates.get(callee_symbol).copied() else {
                    return Ok(None);
                };
                // A local binding shadowing the predicate name wins, exactly as in resolution.
                if let Some(context) = resolution_context
                    && context
                        .local_naming
                        .expression_resolutions
                        .contains_key(&callee_expression.id)
                {
                    return Ok(None);
                }
                let [argument] = arguments.as_slice() else {
                    return Ok(None);
                };
                if argument.name.is_some() {
                    return Ok(None);
                }
                let argument_expression = arena.get(argument.expression);
                let ExpressionKind::Symbol(argument_symbol) = argument_expression.kind else {
                    return Ok(None);
                };
                // The refined key is exactly the one a read of the argument resolves to: the local
                // slot under a naming context; the flat global entry in a context-less state (the
                // fixture drivers), where every binding lives in the global map. A name that is
                // non-local under a context is a package global — winner semantics, not
                // flow-refined — so no refinement.
                let key = match resolution_context {
                    Some(context) => match context
                        .local_naming
                        .expression_resolutions
                        .get(&argument_expression.id)
                    {
                        Some(binding_id) => EnvironmentKey::Local(*binding_id),
                        None => return Ok(None),
                    },
                    None => EnvironmentKey::Global(argument_symbol),
                };
                let Some(binding) = self.environment.get(&key).cloned() else {
                    return Ok(None);
                };
                let entry_type = self.instantiate_type_scheme(&binding.type_scheme)?;
                let entry_type = self.resolve(entry_type)?;
                Ok(
                    refine_guarded_type(&entry_type, predicate).map(|(true_type, false_type)| {
                        GuardRefinement {
                            key,
                            range: binding.range,
                            true_type,
                            false_type,
                        }
                    }),
                )
            }
            _ => Ok(None),
        }
    }

    // Whether an expression never falls through to the code after it: `return`/`break`/`next`, a
    // call to `stop` (by bare name — the `local`/`return` rebinding caveat applies), a block whose
    // last expression diverges, or an `if`/`else` both of whose branches diverge.
    fn expression_diverges(&self, arena: &HirArena, id: ExpressionId) -> bool {
        match &arena.get(id).kind {
            ExpressionKind::Return { .. } | ExpressionKind::Break | ExpressionKind::Next => true,
            ExpressionKind::Block { expressions, .. } => expressions
                .last()
                .is_some_and(|last| self.expression_diverges(arena, *last)),
            ExpressionKind::Call { callee, .. } => matches!(
                &arena.get(*callee).kind,
                ExpressionKind::Symbol(symbol) if Some(*symbol) == self.stop_symbol
            ),
            ExpressionKind::If {
                consequence,
                alternative: Some(alternative),
                ..
            } => {
                self.expression_diverges(arena, *consequence)
                    && self.expression_diverges(arena, *alternative)
            }
            _ => false,
        }
    }

    // A loop body may run zero or more times, so the types flowing around the back edge join into
    // the body's entry environment: `iterate` is re-run until that entry stabilizes, starting from
    // the plain pre-state (real code stabilizes on the second pass). At the pass cap, any slot
    // whose type is still changing (for example one growing structurally each iteration) is
    // widened to `Unknown` as a termination safety net — a strict origin, since the widening
    // introduces a genuine `Unknown`. Afterwards the loop's exit state is applied: `join(pre, out)`
    // for `for`/`while` and for a `repeat` whose body contains `break`/`next` (zero or partial
    // iterations possible), the final out-state for an exit-free `repeat` (runs to completion at
    // least once). The loop variable is re-seeded from the iterable on every pass; after the loop
    // it keeps its final state joined with the pre-loop state (R keeps the last element).
    //
    // Nested loops would re-run an inner region's fixed point once per outer pass (passes^depth):
    // each converged region is memoized by its (read ∪ written keys → entry values) guard, so an
    // enclosing pass whose entry state is unchanged replays the exit effects in O(touched keys).
    fn infer_loop_to_fixed_point(
        &mut self,
        expression: &Expression,
        loop_variable: Option<(EnvironmentKey, CoreType, Range)>,
        runs_at_least_once: bool,
        resolution_context: Option<&ResolutionContext<'_>>,
        mut iterate: impl FnMut(&mut Self) -> Result<(), InferenceError>,
    ) -> Result<(), InferenceError> {
        if let Some(memo) = self.loop_memos.get(&expression.id) {
            let guard_holds = memo
                .guard
                .iter()
                .all(|(key, value)| self.environment.get(key) == value.as_ref());
            if guard_holds {
                let memo = memo.clone();
                for (key, value) in memo.exit_effects {
                    self.set_environment_entry(key, value);
                }
                // Re-recording is deduplicated, so this only restores origins dropped by a
                // discovery pass's truncation.
                for origin in memo.origins {
                    self.record_strict_origin(origin.expression_id, origin.range, origin.kind);
                }
                return Ok(());
            }
        }

        let loop_variable_pre_state = loop_variable
            .as_ref()
            .map(|(key, _, _)| self.environment.get(key).cloned());
        let origins_mark = self.strict_origins.len();
        self.loop_access_logs.push(LoopAccessLog {
            accesses: BTreeMap::new(),
            first_pass: true,
            complete: true,
        });
        let outcome = self.run_loop_passes(
            expression,
            &loop_variable,
            runs_at_least_once,
            resolution_context,
            &mut iterate,
        );
        let access_log = self
            .loop_access_logs
            .pop()
            .unwrap_or_else(|| panic!("loop access log missing for {:?}", expression.id));
        let mut exit_effects = outcome?;

        // The loop variable stays visible after the loop with its final state joined against the
        // pre-loop state (zero iterations leave the previous value or, if there was none, the
        // naming layer's maybe-undefined warning applies).
        if let Some((key, _, _)) = &loop_variable {
            let final_state = exit_effects.get(key).cloned().flatten();
            let joined = self.join_environment_entries(
                loop_variable_pre_state.flatten(),
                final_state,
                expression,
            )?;
            exit_effects.insert(*key, joined);
        }

        for (key, value) in &exit_effects {
            self.set_environment_entry(*key, value.clone());
        }
        if access_log.complete {
            let origins = self.strict_origins[origins_mark..].to_vec();
            self.loop_memos.insert(
                expression.id,
                LoopMemo {
                    guard: access_log.accesses,
                    exit_effects,
                    origins,
                },
            );
        }
        Ok(())
    }

    // The fixed-point passes of `infer_loop_to_fixed_point`, returning the exit effects to apply
    // (the loop variable's post-state is handled by the caller). Split out so the caller can pop
    // the access log on both the success and the error path.
    fn run_loop_passes(
        &mut self,
        expression: &Expression,
        loop_variable: &Option<(EnvironmentKey, CoreType, Range)>,
        runs_at_least_once: bool,
        resolution_context: Option<&ResolutionContext<'_>>,
        iterate: &mut impl FnMut(&mut Self) -> Result<(), InferenceError>,
    ) -> Result<BTreeMap<EnvironmentKey, Option<Binding>>, InferenceError> {
        const LOOP_JOIN_PASSES: usize = 3;

        let mut entry: BTreeMap<EnvironmentKey, Option<Binding>> = BTreeMap::new();
        let mut exit: BTreeMap<EnvironmentKey, Option<Binding>> = BTreeMap::new();
        let mut still_changing: BTreeSet<EnvironmentKey> = BTreeSet::new();
        let mut converged = false;
        for _pass in 0..LOOP_JOIN_PASSES {
            let snapshot = self.environment_snapshot();
            for (key, value) in &entry {
                self.set_environment_entry(*key, value.clone());
            }
            if let Some((key, item_type, range)) = loop_variable {
                self.set_environment_entry(
                    *key,
                    Some(Binding {
                        type_scheme: TypeScheme::monomorphic(item_type.clone()),
                        range: *range,
                    }),
                );
            }
            let result = iterate(self);
            exit = self.environment_rollback(snapshot);
            if let Some(log) = self.loop_access_logs.last_mut() {
                log.first_pass = false;
            }
            result?;

            let mut next_entry = entry.clone();
            for (key, exit_value) in &exit {
                let pre_state = self.environment.get(key).cloned();
                let joined =
                    self.join_environment_entries(pre_state, exit_value.clone(), expression)?;
                next_entry.insert(*key, joined);
            }
            if let Some((key, _, _)) = loop_variable {
                next_entry.remove(key);
            }
            still_changing = next_entry
                .iter()
                .filter(|(key, value)| entry.get(*key) != Some(*value))
                .map(|(key, _)| *key)
                .collect();
            if still_changing.is_empty() {
                converged = true;
                break;
            }
            entry = next_entry;
        }
        if !converged {
            for key in &still_changing {
                let range = entry
                    .get(key)
                    .and_then(|value| value.as_ref())
                    .map(|binding| binding.range)
                    .unwrap_or(expression.range);
                entry.insert(
                    *key,
                    Some(Binding {
                        type_scheme: TypeScheme::monomorphic(CoreType::Unknown),
                        range,
                    }),
                );
                let symbol = match key {
                    EnvironmentKey::Global(symbol) => Some(*symbol),
                    EnvironmentKey::Local(binding_id) => resolution_context.and_then(|context| {
                        context
                            .local_naming
                            .bindings
                            .get(binding_id)
                            .map(|binding| binding.symbol)
                    }),
                };
                if let Some(symbol) = symbol {
                    self.record_strict_origin(
                        expression.id,
                        range,
                        StrictOriginKind::LoopWidened(symbol),
                    );
                }
            }
        }

        let mut exit_effects = BTreeMap::new();
        if runs_at_least_once && converged {
            for (key, value) in exit {
                exit_effects.insert(key, value);
            }
        } else {
            for (key, value) in entry {
                exit_effects.insert(key, value);
            }
            if let Some((key, _, _)) = loop_variable
                && let Some(final_state) = exit.remove(key)
            {
                // `entry` excludes the re-seeded loop variable; its region-final state still
                // feeds the caller's post-loop join.
                exit_effects.insert(*key, final_state);
            }
        }
        Ok(exit_effects)
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_for_expression(
        &mut self,
        _expression_id: ExpressionId,
        variable: Symbol,
        sequence: &Expression,
        body: &Expression,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let range = expression.range;
        // The sequence is evaluated once, before any iteration, so it stays outside the loop
        // region.
        let inferred_sequence = self.infer_expression_with_context(
            sequence,
            arena,
            resolution_context,
            type_definitions,
        )?;
        let sequence_type =
            self.resolve_structural(inferred_sequence, type_definitions, Some(sequence))?;
        let Some(item_type) = iterable_item_type(&sequence_type) else {
            return Err(InferenceError::TypeMismatch {
                expected: Box::new(CoreType::vector(Atomic::Integer)),
                actual: Box::new(sequence_type),
                range: Some(range),
                expression_id: Some(sequence.id),
            });
        };

        let variable_key = match resolution_context.and_then(|context| {
            find_binding(context.local_naming, context.document_id, variable, range)
        }) {
            Some(binding_id) => {
                if let Some(context) = resolution_context {
                    self.note_slot_write(context.local_naming, binding_id, &item_type, expression)?;
                }
                EnvironmentKey::Local(binding_id)
            }
            None => EnvironmentKey::Global(variable),
        };
        self.infer_loop_to_fixed_point(
            expression,
            Some((variable_key, item_type, range)),
            false,
            resolution_context,
            |state| {
                state.infer_expression_with_context(
                    body,
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
                Ok(())
            },
        )?;
        Ok(CoreType::Null)
    }

    fn infer_while_expression(
        &mut self,
        condition: &Expression,
        body: &Expression,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // The condition re-evaluates before every iteration, so it belongs to the iterated region:
        // a read in it sees the types flowing around the back edge.
        self.infer_loop_to_fixed_point(expression, None, false, resolution_context, |state| {
            state.expect_scalar_logical(condition, arena, resolution_context, type_definitions)?;
            state.infer_expression_with_context(
                body,
                arena,
                resolution_context,
                type_definitions,
            )?;
            Ok(())
        })?;
        Ok(CoreType::Null)
    }

    fn infer_repeat_expression(
        &mut self,
        body: &Expression,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // `repeat` runs its body at least once, but a `break`/`next` may leave before the body's
        // end, so only an exit-free body definitely applies all its writes.
        let runs_to_completion = !contains_loop_exit(arena, body.id);
        self.infer_loop_to_fixed_point(
            expression,
            None,
            runs_to_completion,
            resolution_context,
            |state| {
                state.infer_expression_with_context(
                    body,
                    arena,
                    resolution_context,
                    type_definitions,
                )?;
                Ok(())
            },
        )?;
        Ok(CoreType::Null)
    }

    fn expect_scalar_logical(
        &mut self,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<(), InferenceError> {
        let inferred_type = self.infer_expression_with_context(
            expression,
            arena,
            resolution_context,
            type_definitions,
        )?;
        // Project a nominal operand to its representation first, so a nominal whose representation is
        // `logical` is accepted by `&&`/`||` and `if`/`while` conditions, exactly as `!`, arithmetic,
        // and comparison already project nominals.
        let resolved_type =
            self.resolve_structural(inferred_type, type_definitions, Some(expression))?;
        self.unify_with_context(CoreType::Scalar(Atomic::Logical), resolved_type, expression)?;
        Ok(())
    }

    fn check_compatibility(
        &mut self,
        actual_type: CoreType,
        expected_type: CoreType,
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<bool, InferenceError> {
        if self.recursion_depth >= RECURSION_LIMIT {
            return Err(InferenceError::RecursionLimitExceeded);
        }
        self.recursion_depth += 1;
        // Probe the structural check speculatively. A successful check keeps the inference-variable
        // bindings it makes (the two `Variable` arms binding against the other side is how `@new` and
        // checked annotations infer their type arguments), but any false-or-erroring check reverses
        // every mutation. This makes the predicate pure on failure, so it leaks nothing and its result
        // is order-independent. The snapshot does not capture `recursion_depth`.
        let snapshot = self.snapshot();
        let result = self.check_compatibility_inner(
            actual_type,
            expected_type,
            type_definitions,
            expression,
        );
        match &result {
            Ok(true) => self.commit(snapshot),
            Ok(false) | Err(_) => self.rollback_to(snapshot),
        }
        self.recursion_depth -= 1;
        result
    }

    fn check_compatibility_inner(
        &mut self,
        actual_type: CoreType,
        expected_type: CoreType,
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<bool, InferenceError> {
        let actual_type = self.resolve(actual_type)?;
        let expected_type = self.resolve(expected_type)?;

        if expected_type == CoreType::Any || actual_type == CoreType::Any {
            return Ok(true);
        }

        if actual_type == expected_type {
            return Ok(true);
        }

        if let CoreType::Variable(actual_var) = actual_type {
            return Ok(self
                .unify_internal(CoreType::Variable(actual_var), expected_type, None)
                .is_ok());
        }

        if let CoreType::Variable(expected_var) = expected_type {
            return Ok(self
                .unify_internal(actual_type, CoreType::Variable(expected_var), None)
                .is_ok());
        }

        match (actual_type, expected_type) {
            // A union value must be accepted in every shape it can take, so each actual member is
            // checked against the expected type. This arm comes first so union-vs-union reduces to
            // "every actual member fits somewhere in the expected union".
            (CoreType::Union(actual_members), expected_type) => {
                for member in actual_members {
                    if !self.check_compatibility(
                        member,
                        expected_type.clone(),
                        type_definitions,
                        expression,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            // A value fits an expected union when it fits any member. Each attempt is its own
            // probe (`check_compatibility` rolls back failed attempts), so an earlier failing
            // member leaks no bindings into a later one.
            (actual_type, CoreType::Union(expected_members)) => {
                for member in expected_members {
                    if self.check_compatibility(
                        actual_type.clone(),
                        member,
                        type_definitions,
                        expression,
                    )? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            (
                CoreType::Nominal(actual_name, actual_arguments),
                CoreType::Nominal(expected_name, expected_arguments),
            ) if actual_name == expected_name
                && actual_arguments.len() == expected_arguments.len() =>
            {
                // Each type argument is checked in the direction dictated by where the parameter
                // occurs in the representation: covariant for return/container/direct positions,
                // contravariant (flipped) for function-parameter positions, and invariant (both
                // directions) when a parameter occurs in conflicting positions. Without a definition
                // the variance is unknown, so every argument is checked invariantly.
                // A missing definition leaves `variances` empty, so every argument defaults to
                // invariant below. This is conservative: it over-rejects (demands an exact match)
                // rather than over-accepting an unsound widening.
                let variances = type_definitions
                    .get(actual_name)
                    .map(parameter_variances)
                    .unwrap_or_default();

                for (index, (actual_argument, expected_argument)) in actual_arguments
                    .into_iter()
                    .zip(expected_arguments)
                    .enumerate()
                {
                    let variance = variances.get(index).copied().unwrap_or(Variance::Invariant);
                    let compatible = match variance {
                        // The parameter never occurs in the representation, so the argument is
                        // unconstrained and any argument is accepted.
                        Variance::Bivariant => true,
                        Variance::Covariant => self.check_compatibility(
                            actual_argument,
                            expected_argument,
                            type_definitions,
                            expression,
                        )?,
                        Variance::Contravariant => self.check_compatibility(
                            expected_argument,
                            actual_argument,
                            type_definitions,
                            expression,
                        )?,
                        Variance::Invariant => {
                            self.check_compatibility(
                                actual_argument.clone(),
                                expected_argument.clone(),
                                type_definitions,
                                expression,
                            )? && self.check_compatibility(
                                expected_argument,
                                actual_argument,
                                type_definitions,
                                expression,
                            )?
                        }
                    };
                    if !compatible {
                        return Ok(false);
                    }
                }

                Ok(true)
            }
            (CoreType::Nominal(actual_name, actual_arguments), other_type) => {
                let Some(representation_type) = self.nominal_representation_type(
                    actual_name,
                    &actual_arguments,
                    type_definitions,
                    expression,
                )?
                else {
                    return Ok(false);
                };

                self.check_compatibility(
                    representation_type,
                    other_type,
                    type_definitions,
                    expression,
                )
            }
            // A scalar coerces into a vector position, a named vector drops its names into a plain
            // vector position, and vectors check element-wise. Element recursion lands on the
            // scalar arms below for concrete elements (so `integer` widening applies inside
            // vectors too) and on the variable arms above for a generic element (`T[]`), which is
            // how a call like `sort(c(1L))` binds `T := integer`.
            (CoreType::Scalar(actual_atomic), CoreType::Vector(expected_element)) => self
                .check_compatibility(
                    CoreType::Scalar(actual_atomic),
                    *expected_element,
                    type_definitions,
                    expression,
                ),
            (CoreType::NamedVector(actual_element), CoreType::Vector(expected_element)) => self
                .check_compatibility(
                    *actual_element,
                    *expected_element,
                    type_definitions,
                    expression,
                ),
            // `integer` widens to `double` in compatibility (a directional check only — unification
            // never widens): R freely promotes integers in numeric contexts, and without this every
            // numeric parameter in the stub corpus had to be `Any` to avoid rejecting `mean(1L)`.
            (CoreType::Scalar(actual_atomic), CoreType::Scalar(expected_atomic))
                if atomic_widens_to(actual_atomic, expected_atomic) =>
            {
                Ok(true)
            }
            (CoreType::Vector(actual_element), CoreType::Vector(expected_element)) => self
                .check_compatibility(
                    *actual_element,
                    *expected_element,
                    type_definitions,
                    expression,
                ),
            (CoreType::NamedVector(actual_element), CoreType::NamedVector(expected_element)) => {
                self.check_compatibility(
                    *actual_element,
                    *expected_element,
                    type_definitions,
                    expression,
                )
            }
            // Fixed-shape structural compatibility, checked covariantly per element/field. This is
            // what lets `@new` and checked annotations on a `list(...)` accept (and unify) a value
            // whose fields are still inference variables, e.g. `@new Person` on
            // `list(name = name, age = age)` inside an unannotated function.
            (CoreType::Tuple(actual_items), CoreType::Tuple(expected_items))
                if actual_items.len() == expected_items.len() =>
            {
                for (actual_item, expected_item) in actual_items.into_iter().zip(expected_items) {
                    if !self.check_compatibility(
                        actual_item,
                        expected_item,
                        type_definitions,
                        expression,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (CoreType::Record(actual_fields), CoreType::Record(expected_fields))
                if actual_fields.len() == expected_fields.len() =>
            {
                for expected_field in expected_fields {
                    let Some(actual_field) = actual_fields
                        .iter()
                        .find(|field| field.name == expected_field.name)
                    else {
                        return Ok(false);
                    };
                    if !self.check_compatibility(
                        actual_field.value.clone(),
                        expected_field.value,
                        type_definitions,
                        expression,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (CoreType::Tuple(items), CoreType::List(item_type)) => {
                for item in items {
                    if !self.check_compatibility(
                        item,
                        *item_type.clone(),
                        type_definitions,
                        expression,
                    )? {
                        return Ok(false);
                    }
                }

                Ok(true)
            }
            (CoreType::Record(fields), CoreType::List(item_type))
            | (CoreType::Record(fields), CoreType::NamedList(item_type)) => {
                for field in fields {
                    if !self.check_compatibility(
                        field.value,
                        *item_type.clone(),
                        type_definitions,
                        expression,
                    )? {
                        return Ok(false);
                    }
                }

                Ok(true)
            }
            (CoreType::NamedList(actual_item_type), CoreType::List(expected_item_type))
            | (CoreType::NamedList(actual_item_type), CoreType::NamedList(expected_item_type))
            | (CoreType::List(actual_item_type), CoreType::List(expected_item_type)) => self
                .check_compatibility(
                    *actual_item_type,
                    *expected_item_type,
                    type_definitions,
                    expression,
                ),
            (CoreType::Function(actual_function), CoreType::Function(expected_function)) => {
                let actual_parameter_count =
                    actual_function.parameters.len() + actual_function.named_parameters.len();
                let expected_parameter_count =
                    expected_function.parameters.len() + expected_function.named_parameters.len();

                if actual_parameter_count != expected_parameter_count {
                    return Ok(false);
                }

                // Variadic compatibility is conservative: a variadic function is compatible only with
                // another variadic (their rest elements are contravariant, like ordinary parameters),
                // and a variadic/fixed pair is always incompatible. This over-rejects some safe pairings
                // but never admits an unsound one.
                match (&actual_function.variadic, &expected_function.variadic) {
                    (Some(actual_element), Some(expected_element)) => {
                        if !self.check_compatibility(
                            (**expected_element).clone(),
                            (**actual_element).clone(),
                            type_definitions,
                            expression,
                        )? {
                            return Ok(false);
                        }
                    }
                    (None, None) => {}
                    _ => return Ok(false),
                }

                // Parameters pair by NAME where both sides name them (R matches call arguments
                // against formal names, so `fn(a: integer, b: character)` and a function defined
                // `function(b, a)` pair a-with-a and b-with-b regardless of order); unnamed
                // (positional) parameters consume the remaining slots left to right. A named
                // expected parameter with no same-named actual falls back to positional pairing —
                // interface names that do not exist on the actual function are the annotation
                // path's hard error, while plain value flow stays permissive for unnamed shapes.
                let mut actual_parameters = actual_function
                    .parameters
                    .into_iter()
                    .map(|parameter| (None, parameter, false))
                    .collect::<Vec<_>>();
                actual_parameters.extend(
                    actual_function
                        .named_parameters
                        .into_iter()
                        .map(|parameter| {
                            (Some(parameter.name), parameter.value, parameter.optional)
                        }),
                );

                let mut paired: Vec<Option<(CoreType, bool)>> = vec![None; actual_parameters.len()];
                let mut positional_expected: Vec<(CoreType, bool)> = Vec::new();
                for parameter in expected_function.named_parameters {
                    match actual_parameters
                        .iter()
                        .position(|(name, ..)| *name == Some(parameter.name))
                    {
                        Some(index) if paired[index].is_none() => {
                            paired[index] = Some((parameter.value, parameter.optional));
                        }
                        _ => positional_expected.push((parameter.value, parameter.optional)),
                    }
                }
                let mut positional_expected = expected_function
                    .parameters
                    .into_iter()
                    .map(|parameter| (parameter, false))
                    .chain(positional_expected);
                for slot in paired.iter_mut() {
                    if slot.is_none() {
                        *slot = positional_expected.next();
                    }
                }

                for ((_, actual_param, actual_optional), (expected_param, expected_optional)) in
                    actual_parameters
                        .into_iter()
                        .zip(paired.into_iter().map(|slot| {
                            slot.expect("parameter counts were checked equal before pairing")
                        }))
                {
                    // An expected-optional parameter promises callers they may omit it, so
                    // the actual function must have a default for that parameter.
                    if expected_optional && !actual_optional {
                        return Ok(false);
                    }

                    // Parameters are contravariant: a function used where `expected` is wanted
                    // must accept every argument the expected interface may pass, so the expected
                    // parameter type must be compatible with the actual one.
                    if !self.check_compatibility(
                        expected_param,
                        actual_param,
                        type_definitions,
                        expression,
                    )? {
                        return Ok(false);
                    }
                }

                // Return types stay covariant.
                self.check_compatibility(
                    *actual_function.return_type,
                    *expected_function.return_type,
                    type_definitions,
                    expression,
                )
            }
            _ => Ok(false),
        }
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
                if arguments.is_empty()
                    && let Some(core_type) = substitutions.get(name)
                {
                    return Ok(core_type.clone());
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
                    .map(|element| {
                        self.lower_surface_type_with_substitutions(
                            element,
                            substitutions,
                            expanding_aliases,
                            type_definitions,
                            expression,
                        )
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

    // Operators and indexing need a structural shape, and nominal values are compatible
    // with their representation type, so they project through nominal identity here. The
    // seen-set guards against recursive nominal representations.
    fn resolve_structural(
        &mut self,
        core_type: CoreType,
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        let mut resolved_type = self.resolve(core_type)?;
        let mut seen_nominals = BTreeSet::new();

        while let CoreType::Nominal(name, type_arguments) = &resolved_type {
            if !seen_nominals.insert(*name) {
                break;
            }
            let Some(representation_type) = self.nominal_representation_type(
                *name,
                type_arguments,
                type_definitions,
                expression,
            )?
            else {
                break;
            };
            resolved_type = self.resolve(representation_type)?;
        }

        Ok(resolved_type)
    }

    pub fn resolve(&mut self, core_type: CoreType) -> Result<CoreType, InferenceError> {
        if self.recursion_depth >= RECURSION_LIMIT {
            return Err(InferenceError::RecursionLimitExceeded);
        }
        self.recursion_depth += 1;
        let result = self.resolve_inner(core_type);
        self.recursion_depth -= 1;
        result
    }

    fn resolve_inner(&mut self, core_type: CoreType) -> Result<CoreType, InferenceError> {
        match core_type {
            CoreType::Variable(variable) => self.resolve_variable(variable),
            CoreType::Union(members) => {
                let mut resolved_members = Vec::with_capacity(members.len());
                for member in members {
                    resolved_members.push(self.resolve(member)?);
                }
                // Re-normalize: members that resolved to equal types collapse, and a member that
                // resolved to a union flattens.
                Ok(CoreType::union_of(resolved_members))
            }
            CoreType::Nominal(symbol, type_arguments) => {
                let mut resolved_type_arguments = Vec::with_capacity(type_arguments.len());
                for type_argument in type_arguments {
                    resolved_type_arguments.push(self.resolve(type_argument)?);
                }
                Ok(CoreType::Nominal(symbol, resolved_type_arguments))
            }
            CoreType::Vector(element) => {
                let resolved_element = self.resolve(*element)?;
                Ok(CoreType::Vector(Box::new(resolved_element)))
            }
            CoreType::NamedVector(element) => {
                let resolved_element = self.resolve(*element)?;
                Ok(CoreType::NamedVector(Box::new(resolved_element)))
            }
            CoreType::List(item_type) => {
                let resolved_item_type = self.resolve(*item_type)?;
                Ok(CoreType::List(Box::new(resolved_item_type)))
            }
            CoreType::NamedList(item_type) => {
                let resolved_item_type = self.resolve(*item_type)?;
                Ok(CoreType::NamedList(Box::new(resolved_item_type)))
            }
            CoreType::Record(fields) => {
                let mut resolved_fields = Vec::with_capacity(fields.len());
                for field in fields {
                    resolved_fields.push(RecordField::with_optional(
                        field.name,
                        self.resolve(field.value)?,
                        field.optional,
                    ));
                }
                Ok(CoreType::Record(resolved_fields))
            }
            CoreType::Tuple(items) => {
                let mut resolved_items = Vec::with_capacity(items.len());
                for item in items {
                    resolved_items.push(self.resolve(item)?);
                }
                Ok(CoreType::Tuple(resolved_items))
            }
            CoreType::Function(function_type) => {
                let resolved_function_type = self.resolve_function_type(function_type)?;
                Ok(CoreType::Function(resolved_function_type))
            }
            other_type => Ok(other_type),
        }
    }

    pub fn free_type_variables(
        &mut self,
        core_type: &CoreType,
    ) -> Result<BTreeSet<InferenceVariableId>, InferenceError> {
        self.free_type_variables_in_core_type(core_type)
    }

    pub fn unify(&mut self, left: CoreType, right: CoreType) -> Result<CoreType, InferenceError> {
        self.unify_internal(left, right, None)
    }

    pub fn unify_with_context(
        &mut self,
        left: CoreType,
        right: CoreType,
        expression: &Expression,
    ) -> Result<CoreType, InferenceError> {
        self.unify_internal(left, right, Some(expression))
    }

    // Joins two control-flow results into one type. Types that unify share a representative — the
    // probe commits, which is what keeps the chooser idiom `if (c) a else b` linking two inference
    // variables — and genuinely different types fall back to their union, with the failed probe's
    // bindings rolled back so neither side is left constrained by the attempt. A recursion-limit
    // error is resource exhaustion, not a mismatch, so it propagates instead of producing a union.
    fn join_types(
        &mut self,
        left: CoreType,
        right: CoreType,
        expression: &Expression,
    ) -> Result<CoreType, InferenceError> {
        let left = self.resolve(left)?;
        let right = self.resolve(right)?;

        // A `NULL` side joins by pure union, exactly like `if` without `else`: probing unification
        // first would bind an unconstrained inference variable on the other side to `NULL`,
        // collapsing the `T | NULL` results the nullable idioms rely on.
        if left == CoreType::Null || right == CoreType::Null {
            return Ok(CoreType::union_of(vec![left, right]));
        }

        let snapshot = self.snapshot();
        match self.unify_internal(left.clone(), right.clone(), Some(expression)) {
            Ok(unified_type) => {
                self.commit(snapshot);
                self.resolve(unified_type)
            }
            Err(InferenceError::RecursionLimitExceeded) => {
                self.rollback_to(snapshot);
                Err(InferenceError::RecursionLimitExceeded)
            }
            Err(_) => {
                self.rollback_to(snapshot);
                Ok(CoreType::union_of(vec![left, right]))
            }
        }
    }

    fn unify_internal(
        &mut self,
        left: CoreType,
        right: CoreType,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        if self.recursion_depth >= RECURSION_LIMIT {
            return Err(InferenceError::RecursionLimitExceeded);
        }
        self.recursion_depth += 1;
        let result = self.unify_internal_inner(left, right, expression);
        self.recursion_depth -= 1;
        result
    }

    fn unify_internal_inner(
        &mut self,
        left: CoreType,
        right: CoreType,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        let resolved_left = self.resolve(left)?;
        let resolved_right = self.resolve(right)?;

        match (resolved_left, resolved_right) {
            (CoreType::Variable(left_variable), CoreType::Variable(right_variable)) => {
                self.unify_variables(left_variable, right_variable)
            }
            (CoreType::Variable(variable), other_type)
            | (other_type, CoreType::Variable(variable)) => {
                self.bind_variable(variable, other_type.clone(), expression)?;
                Ok(other_type)
            }
            (CoreType::Any, other_type) | (other_type, CoreType::Any) => Ok(other_type),
            (CoreType::Unknown, other_type) | (other_type, CoreType::Unknown) => Ok(other_type),
            (CoreType::Null, CoreType::Null) => Ok(CoreType::Null),
            // Unification is the invariant floor and stays syntactic for unions: no member-wise
            // subtyping search happens here (that is `check_compatibility`'s job). Two unions unify
            // when their member sets are equal (order is presentation, not identity). The one
            // member-wise case kept is the nullable shape `T | NULL` vs `U | NULL` with exactly one
            // non-`NULL` member each — the pairing is unambiguous, and inferring through it is what
            // lets a `<T> ... T | NULL` scheme instantiate against a concrete nullable.
            (CoreType::Union(left_members), CoreType::Union(right_members)) => {
                let left_nullable_inner = nullable_single_member(&left_members);
                let right_nullable_inner = nullable_single_member(&right_members);
                if let (Some(left_inner), Some(right_inner)) =
                    (left_nullable_inner, right_nullable_inner)
                {
                    let unified = self.unify_internal(left_inner, right_inner, expression)?;
                    return Ok(CoreType::union_of(vec![unified, CoreType::Null]));
                }
                let sets_equal = left_members.len() == right_members.len()
                    && left_members
                        .iter()
                        .all(|member| right_members.contains(member));
                if sets_equal {
                    Ok(CoreType::Union(left_members))
                } else {
                    Err(InferenceError::TypeMismatch {
                        expected: Box::new(CoreType::Union(left_members)),
                        actual: Box::new(CoreType::Union(right_members)),
                        range: expression.map(|current_expression| current_expression.range),
                        expression_id: expression.map(|current_expression| current_expression.id),
                    })
                }
            }
            (
                CoreType::Nominal(left_name, left_arguments),
                CoreType::Nominal(right_name, right_arguments),
            ) if left_name == right_name && left_arguments.len() == right_arguments.len() => {
                // Unification is the invariant floor: it must produce a single representative type,
                // so every nominal argument is unified by equality regardless of the parameter's
                // compatibility variance. This is consistent with `check_compatibility` (unified ⇒
                // compatible in both directions): unify is strictly stronger than compatibility.
                let mut unified_arguments = Vec::with_capacity(left_arguments.len());
                for (left_argument, right_argument) in
                    left_arguments.into_iter().zip(right_arguments)
                {
                    unified_arguments.push(self.unify_internal(
                        left_argument,
                        right_argument,
                        expression,
                    )?);
                }
                Ok(CoreType::Nominal(left_name, unified_arguments))
            }
            (CoreType::Scalar(left_atomic), CoreType::Scalar(right_atomic))
                if left_atomic == right_atomic =>
            {
                Ok(CoreType::Scalar(left_atomic))
            }
            (CoreType::Vector(left_element), CoreType::Vector(right_element)) => {
                let unified_element =
                    self.unify_internal(*left_element, *right_element, expression)?;
                Ok(CoreType::Vector(Box::new(unified_element)))
            }
            (CoreType::NamedVector(left_element), CoreType::NamedVector(right_element)) => {
                let unified_element =
                    self.unify_internal(*left_element, *right_element, expression)?;
                Ok(CoreType::NamedVector(Box::new(unified_element)))
            }
            (CoreType::List(left_item_type), CoreType::List(right_item_type)) => {
                let unified_item_type =
                    self.unify_internal(*left_item_type, *right_item_type, expression)?;
                Ok(CoreType::List(Box::new(unified_item_type)))
            }
            (CoreType::NamedList(left_item_type), CoreType::NamedList(right_item_type)) => {
                let unified_item_type =
                    self.unify_internal(*left_item_type, *right_item_type, expression)?;
                Ok(CoreType::NamedList(Box::new(unified_item_type)))
            }
            (CoreType::Tuple(left_items), CoreType::Tuple(right_items)) => {
                self.unify_tuples(left_items, right_items, expression)
            }
            (CoreType::Record(left_fields), CoreType::Record(right_fields)) => {
                self.unify_records(left_fields, right_fields, expression)
            }
            (CoreType::Function(left_function), CoreType::Function(right_function)) => {
                let unified_function =
                    self.unify_functions(left_function, right_function, expression)?;
                Ok(CoreType::Function(unified_function))
            }
            (left_type, right_type) => Err(InferenceError::TypeMismatch {
                expected: Box::new(left_type),
                actual: Box::new(right_type),
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            }),
        }
    }

    pub fn occurs_in(
        &mut self,
        variable: InferenceVariableId,
        core_type: &CoreType,
    ) -> Result<bool, InferenceError> {
        match self.resolve(core_type.clone())? {
            CoreType::Variable(other_variable) => Ok(variable == other_variable),
            CoreType::Union(members) => {
                for member in members {
                    if self.occurs_in(variable, &member)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CoreType::Nominal(_, type_arguments) => {
                for type_argument in type_arguments {
                    if self.occurs_in(variable, &type_argument)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CoreType::Vector(element) => self.occurs_in(variable, &element),
            CoreType::NamedVector(element) => self.occurs_in(variable, &element),
            CoreType::List(item_type) => self.occurs_in(variable, &item_type),
            CoreType::NamedList(item_type) => self.occurs_in(variable, &item_type),
            CoreType::Record(fields) => {
                for field in fields {
                    if self.occurs_in(variable, &field.value)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CoreType::Tuple(items) => {
                for item in items {
                    if self.occurs_in(variable, &item)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CoreType::Function(function_type) => {
                for parameter in function_type.parameters {
                    if self.occurs_in(variable, &parameter)? {
                        return Ok(true);
                    }
                }

                for named_parameter in function_type.named_parameters {
                    if self.occurs_in(variable, &named_parameter.value)? {
                        return Ok(true);
                    }
                }

                if let Some(element) = &function_type.variadic
                    && self.occurs_in(variable, element)?
                {
                    return Ok(true);
                }

                self.occurs_in(variable, &function_type.return_type)
            }
            _ => Ok(false),
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
                // A flexible endpoint such as `1:n` is constrained numeric but is not known to be
                // `integer`; it may resolve to `double`, so the result must be `double[]`. Claiming
                // `integer[]` here is unsound when the endpoint instantiates at `double`.
                CoreType::Variable(variable) => {
                    self.constrain_type(
                        CoreType::Variable(variable),
                        Constraint::Numeric,
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

    fn infer_function_call_expression(
        &mut self,
        callee: &Expression,
        arguments: &[Argument],
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        // An overloaded stub callee resolves per call site: each scheme is probed in declaration
        // order and the first whose parameters accept the arguments wins (its return is the call's
        // type). Only a plain or namespace-qualified name can be overloaded, and a local binding
        // shadowing the name disables the set (the local wins, as everywhere).
        if let Some(overload_symbol) = callee_overload_symbol(callee, resolution_context)
            && let Some(schemes) = self.overload_sets.get(&overload_symbol).cloned()
            && schemes.len() > 1
        {
            return self.infer_overloaded_call(
                overload_symbol,
                &schemes,
                arguments,
                callee,
                expression,
                arena,
                resolution_context,
                type_definitions,
            );
        }

        let inferred_callee = self.infer_expression_with_context(
            callee,
            arena,
            resolution_context,
            type_definitions,
        )?;
        let resolved_callee = self.resolve(inferred_callee)?;

        match resolved_callee {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::Variable(variable) => {
                let mut positional_arguments = Vec::new();
                let mut named_arguments = Vec::new();

                for argument in arguments {
                    let inferred_argument = self.infer_expression_with_context(
                        arena.get(argument.expression),
                        arena,
                        resolution_context,
                        type_definitions,
                    )?;
                    if let Some(name) = argument.name {
                        named_arguments.push(RecordField::new(name, inferred_argument));
                    } else {
                        positional_arguments.push(inferred_argument);
                    }
                }

                let return_variable = self.fresh_variable();
                self.unify_with_context(
                    CoreType::Variable(variable),
                    CoreType::Function(FunctionType::new(
                        positional_arguments,
                        named_arguments,
                        CoreType::Variable(return_variable),
                    )),
                    expression,
                )?;
                self.resolve(CoreType::Variable(return_variable))
            }
            CoreType::Function(function_type) => self.infer_function_call(
                function_type,
                arguments,
                callee,
                expression,
                arena,
                resolution_context,
                type_definitions,
            ),
            other_type => Err(InferenceError::ExpectedFunction {
                actual_type: Box::new(other_type),
                range: callee.range,
                expression_id: callee.id,
            }),
        }
    }

    // Probes each scheme of an overloaded name in declaration order and commits the first one whose
    // signature accepts the arguments; its return type is the call's type. Arguments are inferred
    // exactly once, before any probe: expression inference writes fields the probe snapshot does not
    // reverse (`environment`, `recorded_expression_types`), so running it inside a probe would leak
    // bindings that reference rolled-back variable ids. The probes themselves run only the
    // instantiation and argument-matching paths, which stay within the snapshot contract.
    #[allow(clippy::too_many_arguments)]
    fn infer_overloaded_call(
        &mut self,
        symbol: Symbol,
        schemes: &[TypeScheme],
        arguments: &[Argument],
        callee: &Expression,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let argument_types =
            self.infer_call_arguments(arguments, arena, resolution_context, type_definitions)?;

        // Selection needs concrete argument types. Probing against an argument whose type still
        // contains a free inference variable would let the first candidate bind it — committing a
        // wrapper function's parameter (`function(x) sum(x)`) to the first scheme's parameter type
        // and rejecting calls R accepts. Such a call skips selection and uses the final
        // declaration, by corpus convention the most general one.
        let mut has_unresolved_argument = false;
        for argument_type in &argument_types {
            if !self.free_type_variables(argument_type)?.is_empty() {
                has_unresolved_argument = true;
                break;
            }
        }
        let declared_count = schemes.len();
        let schemes = match (has_unresolved_argument, schemes.split_last()) {
            (true, Some((last, _))) => std::slice::from_ref(last),
            _ => schemes,
        };
        // Maps a probe index back into the declared set: the unresolved-argument fallback probes
        // only the final declaration, so its one candidate is the set's last index.
        let declared_index = |probe_index: usize| {
            if schemes.len() == declared_count {
                probe_index
            } else {
                declared_count - 1
            }
        };

        // Selection runs strict first, then (only if nothing matched and a whole-number double
        // literal is present) once more with the literal-as-integer courtesy. During the strict
        // round the courtesy is off (`overload_probe_depth`): `1` is genuinely a double at runtime,
        // so letting it match an integer candidate would pick a signature whose return type
        // misstates what R computes (`sum(1, 2)` is a double, not an integer). The courtesy round
        // keeps a name whose only fitting candidate wants `integer` callable as `foo(1)` — exact
        // matches outrank conversions.
        let literal_courtesy_rounds: &[bool] = if arguments
            .iter()
            .any(|argument| is_whole_number_double_literal(arena.get(argument.expression)))
        {
            &[false, true]
        } else {
            &[false]
        };

        let mut first_error = None;
        for &allow_literal_courtesy in literal_courtesy_rounds {
            for (probe_index, scheme) in schemes.iter().enumerate() {
                let snapshot = self.snapshot();
                let function_type = match self
                    .instantiate_type_scheme(scheme)
                    .and_then(|instantiated| self.resolve(instantiated))
                {
                    Ok(CoreType::Function(function_type)) => function_type,
                    Ok(_) => {
                        self.rollback_to(snapshot);
                        continue;
                    }
                    Err(error) => {
                        self.rollback_to(snapshot);
                        return Err(error);
                    }
                };
                if !allow_literal_courtesy {
                    self.overload_probe_depth += 1;
                }
                let outcome = self.match_call_arguments(
                    function_type,
                    arguments,
                    &argument_types,
                    callee,
                    expression,
                    arena,
                    type_definitions,
                );
                if !allow_literal_courtesy {
                    self.overload_probe_depth -= 1;
                }
                match outcome {
                    Ok(result) => {
                        self.commit(snapshot);
                        if self.record_expression_types {
                            self.selected_overloads
                                .insert(callee.id, declared_index(probe_index));
                        }
                        return Ok(result);
                    }
                    Err(InferenceError::RecursionLimitExceeded) => {
                        self.rollback_to(snapshot);
                        return Err(InferenceError::RecursionLimitExceeded);
                    }
                    Err(error) => {
                        self.rollback_to(snapshot);
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
        }

        // The unresolved-argument fallback probes a single scheme; failing it is an ordinary call
        // mismatch, so the underlying error reads better than a one-candidate overload report.
        if schemes.len() == 1
            && let Some(error) = first_error
        {
            return Err(error);
        }

        Err(InferenceError::NoMatchingOverload {
            symbol,
            candidate_count: schemes.len(),
            range: expression.range,
            expression_id: expression.id,
            first_error: first_error.map(Box::new),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_function_call(
        &mut self,
        function_type: FunctionType<CoreType>,
        arguments: &[Argument],
        callee: &Expression,
        expression: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let argument_types =
            self.infer_call_arguments(arguments, arena, resolution_context, type_definitions)?;
        self.match_call_arguments(
            function_type,
            arguments,
            &argument_types,
            callee,
            expression,
            arena,
            type_definitions,
        )
    }

    fn infer_call_arguments(
        &mut self,
        arguments: &[Argument],
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<Vec<CoreType>, InferenceError> {
        arguments
            .iter()
            .map(|argument| {
                self.infer_expression_with_context(
                    arena.get(argument.expression),
                    arena,
                    resolution_context,
                    type_definitions,
                )
            })
            .collect()
    }

    // Matches already-inferred argument types against a concrete signature: positionals in order,
    // named arguments by name, surplus positionals into optional named parameters or `...`. Kept
    // free of expression inference so an overload probe can run it inside a snapshot (see
    // `infer_overloaded_call`). `argument_types` is parallel to `arguments`.
    #[allow(clippy::too_many_arguments)]
    fn match_call_arguments(
        &mut self,
        function_type: FunctionType<CoreType>,
        arguments: &[Argument],
        argument_types: &[CoreType],
        callee: &Expression,
        expression: &Expression,
        arena: &HirArena,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        let total_parameters =
            function_type.parameters.len() + function_type.named_parameters.len();
        let required_parameters = function_type.parameters.len()
            + function_type
                .named_parameters
                .iter()
                .filter(|parameter| !parameter.optional)
                .count();
        let expected_named_parameters = function_type
            .named_parameters
            .iter()
            .map(|parameter| parameter.name)
            .collect::<Vec<_>>();
        let actual_named_arguments = arguments
            .iter()
            .filter_map(|argument| argument.name)
            .collect::<Vec<_>>();

        let positional_parameters = function_type.parameters;
        let variadic_element = function_type.variadic.map(|element| *element);
        let return_type = *function_type.return_type;
        let mut next_positional_index = 0;
        let mut remaining_named_parameters = function_type.named_parameters;

        for (argument, inferred_argument) in arguments.iter().zip(argument_types) {
            let arg_expr = arena.get(argument.expression);
            let inferred_argument = inferred_argument.clone();
            if let Some(name) = argument.name {
                let Some(parameter_index) = remaining_named_parameters
                    .iter()
                    .position(|parameter| parameter.name == name)
                else {
                    // A named argument matching no declared parameter is absorbed by the rest
                    // parameter, checked against its element type (R collects unmatched keywords
                    // into `...` — the pass-through idiom `read.csv(f, colClasses = ...)`). A name
                    // that *duplicates* a declared parameter already given stays an error (R:
                    // "formal argument matched by multiple actual arguments"), and without a rest
                    // parameter an unmatched name is an error as before.
                    if let Some(element) = &variadic_element
                        && !expected_named_parameters.contains(&name)
                    {
                        self.check_argument(
                            element.clone(),
                            inferred_argument,
                            arg_expr,
                            type_definitions,
                        )?;
                        continue;
                    }
                    return Err(InferenceError::NamedParameterMismatch {
                        expected_parameters: expected_named_parameters,
                        actual_parameters: actual_named_arguments,
                        range: Some(expression.range),
                        expression_id: Some(expression.id),
                    });
                };

                let parameter = remaining_named_parameters.remove(parameter_index);
                self.check_argument(
                    parameter.value,
                    inferred_argument,
                    arg_expr,
                    type_definitions,
                )?;
                continue;
            }

            if let Some(parameter) = positional_parameters.get(next_positional_index) {
                next_positional_index += 1;
                self.check_argument(
                    parameter.clone(),
                    inferred_argument,
                    arg_expr,
                    type_definitions,
                )?;
                continue;
            }

            // A positional argument past the fixed positionals fills an optional named parameter by
            // position — but only when the function is not variadic. In a variadic function, surplus
            // positionals belong to `...` (R's rule: a named parameter after `...` is matched by name
            // only), so they are absorbed below instead of consuming an optional named parameter.
            if variadic_element.is_none() && !remaining_named_parameters.is_empty() {
                let parameter = remaining_named_parameters.remove(0);
                self.check_argument(
                    parameter.value,
                    inferred_argument,
                    arg_expr,
                    type_definitions,
                )?;
                continue;
            }

            // A variadic function absorbs any number of surplus positional arguments, each checked
            // against the rest-parameter element type. Cloning the element per argument keeps the check
            // order-independent — no argument's check mutates state a later one reads.
            if let Some(element) = &variadic_element {
                self.check_argument(
                    element.clone(),
                    inferred_argument,
                    arg_expr,
                    type_definitions,
                )?;
                continue;
            }

            return Err(InferenceError::FunctionArityMismatch {
                expected: total_parameters,
                actual: arguments.len(),
                range: Some(callee.range),
                expression_id: Some(callee.id),
            });
        }

        if next_positional_index != positional_parameters.len()
            || remaining_named_parameters
                .iter()
                .any(|parameter| !parameter.optional)
        {
            return Err(InferenceError::FunctionArityMismatch {
                expected: required_parameters,
                actual: arguments.len(),
                range: Some(callee.range),
                expression_id: Some(callee.id),
            });
        }

        self.resolve(return_type)
    }

    // Arguments are checked with compatibility, not unification, so coercions like
    // scalar-to-vector and `T` into `T | NULL` work at parameter positions. An `Unknown`
    // argument is accepted to avoid cascading a second error after the cause was already
    // diagnosed where the value became `Unknown`.
    fn check_argument(
        &mut self,
        parameter_type: CoreType,
        argument_type: CoreType,
        argument_expression: &Expression,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<(), InferenceError> {
        let resolved_argument = self.resolve(argument_type)?;
        if resolved_argument == CoreType::Unknown {
            return Ok(());
        }

        if self.check_compatibility(
            resolved_argument.clone(),
            parameter_type.clone(),
            type_definitions,
            Some(argument_expression),
        )? {
            return Ok(());
        }

        // R programmers write `seq_len(10)`, not `seq_len(10L)`: a whole-number double literal
        // counts as an integer at a parameter position, the same rule `:` applies to its
        // endpoints. The retry goes through full compatibility, so integer-expecting unions and
        // vector parameters admit the literal too. Off during a strict overload probe — the
        // courtesy must not decide which candidate wins (see `infer_overloaded_call`).
        if self.overload_probe_depth == 0
            && resolved_argument == CoreType::Scalar(Atomic::Double)
            && is_whole_number_double_literal(argument_expression)
            && self.check_compatibility(
                CoreType::Scalar(Atomic::Integer),
                parameter_type.clone(),
                type_definitions,
                Some(argument_expression),
            )?
        {
            return Ok(());
        }

        // A numeric-constrained parameter rejected the argument because it is not numeric; report
        // that directly rather than rendering the bare inference variable as the expected type.
        let resolved_parameter = self.resolve(parameter_type)?;
        if let CoreType::Variable(variable) = resolved_parameter
            && matches!(
                self.entries.get(&variable),
                Some(InferenceEntry::Unbound {
                    constraint: Constraint::Numeric,
                    ..
                })
            )
        {
            return Err(InferenceError::ConstraintViolation {
                constraint: Constraint::Numeric,
                actual: Box::new(resolved_argument),
                range: Some(argument_expression.range),
                expression_id: Some(argument_expression.id),
            });
        }

        Err(InferenceError::TypeMismatch {
            expected: Box::new(resolved_parameter),
            actual: Box::new(resolved_argument),
            range: Some(argument_expression.range),
            expression_id: Some(argument_expression.id),
        })
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
            CoreType::NamedList(item_type) => {
                if literal_name_symbol(index_expression).is_some() {
                    Ok(nullable_type(*item_type))
                } else {
                    Err(InferenceError::NonLiteralSubscript {
                        container: Box::new(CoreType::NamedList(item_type)),
                        by: SubscriptKind::FieldName,
                        range: expression.range,
                        expression_id: expression.id,
                    })
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
            CoreType::NamedVector(element) => Ok(nullable_type(*element)),
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

    fn resolve_variable(
        &mut self,
        variable: InferenceVariableId,
    ) -> Result<CoreType, InferenceError> {
        let Some(entry) = self.entries.get(&variable).cloned() else {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        };

        match entry {
            InferenceEntry::Unbound { .. } => Ok(CoreType::Variable(variable)),
            InferenceEntry::Redirect(other_variable) => {
                let resolved_type = self.resolve_variable(other_variable)?;
                self.compress_variable(variable, &resolved_type)?;
                Ok(resolved_type)
            }
            InferenceEntry::Bound(bound_type) => {
                let resolved_type = self.resolve(bound_type)?;
                self.compress_variable(variable, &resolved_type)?;
                Ok(resolved_type)
            }
        }
    }

    fn compress_variable(
        &mut self,
        variable: InferenceVariableId,
        resolved_type: &CoreType,
    ) -> Result<(), InferenceError> {
        if !self.entries.contains_key(&variable) {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        }

        let compressed_entry = match resolved_type {
            CoreType::Variable(other_variable) if *other_variable != variable => {
                InferenceEntry::Redirect(*other_variable)
            }
            other_type => InferenceEntry::Bound(other_type.clone()),
        };
        self.set_entry(variable, compressed_entry);

        Ok(())
    }

    fn bind_variable(
        &mut self,
        variable: InferenceVariableId,
        core_type: CoreType,
        expression: Option<&Expression>,
    ) -> Result<(), InferenceError> {
        if self.occurs_in(variable, &core_type)? {
            return Err(InferenceError::OccursCheckFailed {
                variable,
                in_type: Box::new(core_type),
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            });
        }

        // A rigid (skolem) variable models a universally quantified annotation parameter; binding it
        // to a concrete type would specialize a `<T>` the body promised to handle for every T.
        if self.rigid_variables.contains_key(&variable) {
            return Err(InferenceError::TypeMismatch {
                expected: Box::new(self.rigid_display(variable)),
                actual: Box::new(core_type),
                range: expression.map(|current| current.range),
                expression_id: expression.map(|current| current.id),
            });
        }

        let Some(entry) = self.entries.get(&variable).cloned() else {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        };
        if let InferenceEntry::Unbound { level, constraint } = entry {
            // A constrained variable may only be bound to a type that satisfies its bound. When
            // the bound type is itself a variable the constraint propagates there instead, and
            // when it is concrete and unsatisfying `constrain_type` reports the violation.
            self.constrain_type(core_type.clone(), constraint, expression)?;
            // Anything reachable from the bound type escapes to this variable's scope, so
            // inner variables drop to its level and stay monomorphic there.
            self.lower_levels_to(&core_type, level)?;
        }

        if !self.entries.contains_key(&variable) {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        }
        self.set_entry(variable, InferenceEntry::Bound(core_type));
        Ok(())
    }

    fn lower_levels_to(
        &mut self,
        core_type: &CoreType,
        level: Level,
    ) -> Result<(), InferenceError> {
        for variable in self.free_type_variables_in_core_type(core_type)? {
            let lowered_entry = match self.entries.get(&variable) {
                None => return Err(InferenceError::UnknownInferenceVariable(variable)),
                Some(InferenceEntry::Unbound {
                    level: variable_level,
                    constraint,
                }) if *variable_level > level => Some(InferenceEntry::Unbound {
                    level,
                    constraint: *constraint,
                }),
                Some(_) => None,
            };
            if let Some(entry) = lowered_entry {
                self.set_entry(variable, entry);
            }
        }
        Ok(())
    }

    fn unify_variables(
        &mut self,
        left: InferenceVariableId,
        right: InferenceVariableId,
    ) -> Result<CoreType, InferenceError> {
        if left == right {
            return Ok(CoreType::Variable(left));
        }

        let Some(left_entry) = self.entries.get(&left) else {
            return Err(InferenceError::UnknownInferenceVariable(left));
        };
        if !matches!(left_entry, InferenceEntry::Unbound { .. }) {
            let resolved_left = self.resolve_variable(left)?;
            let resolved_right = self.resolve_variable(right)?;
            return self.unify(resolved_left, resolved_right);
        }

        let Some(right_entry) = self.entries.get(&right) else {
            return Err(InferenceError::UnknownInferenceVariable(right));
        };
        if !matches!(right_entry, InferenceEntry::Unbound { .. }) {
            let resolved_left = self.resolve_variable(left)?;
            let resolved_right = self.resolve_variable(right)?;
            return self.unify(resolved_left, resolved_right);
        }

        // Two distinct skolems are different universals and cannot be unified. When exactly one side
        // is rigid it must survive the union so its identity (and rigidity) is preserved; the
        // flexible variable redirects to it.
        let left_rigid = self.rigid_variables.contains_key(&left);
        let right_rigid = self.rigid_variables.contains_key(&right);
        if left_rigid && right_rigid {
            return Err(InferenceError::TypeMismatch {
                expected: Box::new(self.rigid_display(left)),
                actual: Box::new(self.rigid_display(right)),
                range: None,
                expression_id: None,
            });
        }
        let (survivor, redirected) = if left_rigid {
            (left, right)
        } else {
            (right, left)
        };

        let (redirected_level, redirected_constraint) = match self.entries.get(&redirected) {
            Some(InferenceEntry::Unbound { level, constraint }) => (*level, *constraint),
            _ => return Err(InferenceError::UnknownInferenceVariable(redirected)),
        };
        let merged_survivor = match self.entries.get(&survivor) {
            Some(InferenceEntry::Unbound { level, constraint }) => Some(InferenceEntry::Unbound {
                level: (*level).min(redirected_level),
                constraint: (*constraint).join(redirected_constraint),
            }),
            _ => None,
        };
        if let Some(entry) = merged_survivor {
            self.set_entry(survivor, entry);
        }

        if !self.entries.contains_key(&redirected) {
            return Err(InferenceError::UnknownInferenceVariable(redirected));
        }
        self.set_entry(redirected, InferenceEntry::Redirect(survivor));

        Ok(CoreType::Variable(survivor))
    }

    // Interface schemes computed by another `InferenceState` carry variable ids that mean
    // nothing here, so importing re-binds quantified variables to fresh local ids and erases
    // any stray free variable to `Unknown`.
    pub fn import_scheme(&mut self, type_scheme: &TypeScheme) -> TypeScheme {
        let mut substitutions = BTreeMap::new();
        let mut quantified_variables = Vec::with_capacity(type_scheme.quantified_variables.len());
        for quantified in &type_scheme.quantified_variables {
            let fresh = self.fresh_constrained_variable(quantified.constraint);
            substitutions.insert(quantified.variable, fresh);
            quantified_variables.push(QuantifiedVariable::new(fresh, quantified.constraint));
        }

        TypeScheme {
            quantified_variables,
            body: import_core_type(&type_scheme.body, &substitutions),
        }
    }

    fn instantiate_type_scheme(
        &mut self,
        type_scheme: &TypeScheme,
    ) -> Result<CoreType, InferenceError> {
        let mut substitutions = BTreeMap::new();

        for quantified in &type_scheme.quantified_variables {
            substitutions.insert(
                quantified.variable,
                self.fresh_constrained_variable(quantified.constraint),
            );
        }

        self.instantiate_core_type(&type_scheme.body, &substitutions)
    }

    fn instantiate_core_type(
        &mut self,
        core_type: &CoreType,
        substitutions: &BTreeMap<InferenceVariableId, InferenceVariableId>,
    ) -> Result<CoreType, InferenceError> {
        match core_type {
            CoreType::Any => Ok(CoreType::Any),
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Null => Ok(CoreType::Null),
            CoreType::Union(members) => {
                let mut instantiated_members = Vec::with_capacity(members.len());
                for member in members {
                    instantiated_members.push(self.instantiate_core_type(member, substitutions)?);
                }
                Ok(CoreType::union_of(instantiated_members))
            }
            CoreType::Scalar(atomic) => Ok(CoreType::Scalar(*atomic)),
            CoreType::Nominal(symbol, type_arguments) => {
                let mut instantiated_type_arguments = Vec::with_capacity(type_arguments.len());
                for type_argument in type_arguments {
                    instantiated_type_arguments
                        .push(self.instantiate_core_type(type_argument, substitutions)?);
                }
                Ok(CoreType::Nominal(*symbol, instantiated_type_arguments))
            }
            CoreType::Vector(element) => Ok(CoreType::Vector(Box::new(
                self.instantiate_core_type(element, substitutions)?,
            ))),
            CoreType::NamedVector(element) => Ok(CoreType::NamedVector(Box::new(
                self.instantiate_core_type(element, substitutions)?,
            ))),
            CoreType::List(item_type) => Ok(CoreType::List(Box::new(
                self.instantiate_core_type(item_type, substitutions)?,
            ))),
            CoreType::NamedList(item_type) => Ok(CoreType::NamedList(Box::new(
                self.instantiate_core_type(item_type, substitutions)?,
            ))),
            CoreType::Record(fields) => {
                let mut instantiated_fields = Vec::with_capacity(fields.len());
                for field in fields {
                    instantiated_fields.push(RecordField::with_optional(
                        field.name,
                        self.instantiate_core_type(&field.value, substitutions)?,
                        field.optional,
                    ));
                }
                Ok(CoreType::Record(instantiated_fields))
            }
            CoreType::Tuple(items) => {
                let mut instantiated_items = Vec::with_capacity(items.len());
                for item in items {
                    instantiated_items.push(self.instantiate_core_type(item, substitutions)?);
                }
                Ok(CoreType::Tuple(instantiated_items))
            }
            CoreType::Function(function_type) => {
                let mut instantiated_parameters =
                    Vec::with_capacity(function_type.parameters.len());
                for parameter in &function_type.parameters {
                    instantiated_parameters
                        .push(self.instantiate_core_type(parameter, substitutions)?);
                }

                let mut instantiated_named_parameters =
                    Vec::with_capacity(function_type.named_parameters.len());
                for named_parameter in &function_type.named_parameters {
                    instantiated_named_parameters.push(RecordField::with_optional(
                        named_parameter.name,
                        self.instantiate_core_type(&named_parameter.value, substitutions)?,
                        named_parameter.optional,
                    ));
                }

                let instantiated_variadic = match &function_type.variadic {
                    Some(element) => Some(self.instantiate_core_type(element, substitutions)?),
                    None => None,
                };

                let instantiated_return_type =
                    self.instantiate_core_type(&function_type.return_type, substitutions)?;

                Ok(CoreType::Function(FunctionType::with_variadic(
                    instantiated_parameters,
                    instantiated_named_parameters,
                    instantiated_variadic,
                    instantiated_return_type,
                )))
            }
            CoreType::Variable(variable) => Ok(substitutions
                .get(variable)
                .copied()
                .map(CoreType::Variable)
                .unwrap_or(CoreType::Variable(*variable))),
        }
    }

    // Binds numeric-constrained variables reachable outside a function type to `double`. A numeric
    // variable only stays polymorphic when a function parameter abstracts it; anywhere else there
    // is no caller to choose the concrete numeric type, so it defaults like R's bare numbers.
    fn default_free_numeric(&mut self, core_type: CoreType) -> Result<CoreType, InferenceError> {
        let resolved_type = self.resolve(core_type)?;
        match resolved_type {
            CoreType::Variable(variable) => {
                // Only default variables owned by the binding being finalized (created at a deeper
                // level). A numeric variable that escaped from an enclosing scope, such as an outer
                // function parameter referenced by a local binding, stays polymorphic until its own
                // boundary, matching the generalization level rule.
                if matches!(
                    self.entries.get(&variable),
                    Some(InferenceEntry::Unbound {
                        level,
                        constraint: Constraint::Numeric | Constraint::ScalarNumeric,
                    }) if *level > self.current_level
                ) {
                    self.bind_variable(variable, CoreType::Scalar(Atomic::Double), None)?;
                    return self.resolve(CoreType::Variable(variable));
                }
                Ok(CoreType::Variable(variable))
            }
            CoreType::Union(members) => {
                let mut defaulted_members = Vec::with_capacity(members.len());
                for member in members {
                    defaulted_members.push(self.default_free_numeric(member)?);
                }
                Ok(CoreType::union_of(defaulted_members))
            }
            CoreType::Vector(element) => Ok(CoreType::Vector(Box::new(
                self.default_free_numeric(*element)?,
            ))),
            CoreType::NamedVector(element) => Ok(CoreType::NamedVector(Box::new(
                self.default_free_numeric(*element)?,
            ))),
            CoreType::List(item_type) => Ok(CoreType::List(Box::new(
                self.default_free_numeric(*item_type)?,
            ))),
            CoreType::NamedList(item_type) => Ok(CoreType::NamedList(Box::new(
                self.default_free_numeric(*item_type)?,
            ))),
            CoreType::Record(fields) => {
                let mut defaulted_fields = Vec::with_capacity(fields.len());
                for field in fields {
                    defaulted_fields.push(RecordField::with_optional(
                        field.name,
                        self.default_free_numeric(field.value)?,
                        field.optional,
                    ));
                }
                Ok(CoreType::Record(defaulted_fields))
            }
            CoreType::Tuple(items) => {
                let mut defaulted_items = Vec::with_capacity(items.len());
                for item in items {
                    defaulted_items.push(self.default_free_numeric(item)?);
                }
                Ok(CoreType::Tuple(defaulted_items))
            }
            CoreType::Nominal(symbol, type_arguments) => {
                let mut defaulted_arguments = Vec::with_capacity(type_arguments.len());
                for type_argument in type_arguments {
                    defaulted_arguments.push(self.default_free_numeric(type_argument)?);
                }
                Ok(CoreType::Nominal(symbol, defaulted_arguments))
            }
            // Function parameter and return positions keep their numeric variables for
            // generalization, so descending into them would wrongly monomorphize them.
            other_type => Ok(other_type),
        }
    }

    // Quantifies the variables whose level is deeper than the current one: those were
    // created while inferring the binding's value and cannot escape it. Variables shared
    // with the enclosing scope were lowered to its level when they were unified, so no
    // environment walk is needed.
    fn generalize(&mut self, core_type: CoreType) -> Result<TypeScheme, InferenceError> {
        let resolved_type = self.resolve(core_type)?;
        let type_variables = self.free_type_variables_in_core_type(&resolved_type)?;

        let mut quantified_variables = Vec::new();
        for variable in type_variables {
            let Some(entry) = self.entries.get(&variable) else {
                return Err(InferenceError::UnknownInferenceVariable(variable));
            };
            if let InferenceEntry::Unbound { level, constraint } = entry
                && *level > self.current_level
            {
                quantified_variables.push(QuantifiedVariable::new(variable, *constraint));
            }
        }

        Ok(TypeScheme {
            quantified_variables,
            body: resolved_type,
        })
    }

    fn free_type_variables_in_core_type(
        &mut self,
        core_type: &CoreType,
    ) -> Result<BTreeSet<InferenceVariableId>, InferenceError> {
        if self.recursion_depth >= RECURSION_LIMIT {
            return Err(InferenceError::RecursionLimitExceeded);
        }
        self.recursion_depth += 1;
        let result = self.free_type_variables_in_core_type_inner(core_type);
        self.recursion_depth -= 1;
        result
    }

    fn free_type_variables_in_core_type_inner(
        &mut self,
        core_type: &CoreType,
    ) -> Result<BTreeSet<InferenceVariableId>, InferenceError> {
        match self.resolve(core_type.clone())? {
            CoreType::Any | CoreType::Unknown | CoreType::Null | CoreType::Scalar(_) => {
                Ok(BTreeSet::new())
            }
            CoreType::Vector(element) | CoreType::NamedVector(element) => {
                self.free_type_variables_in_core_type(&element)
            }
            CoreType::Union(members) => {
                let mut free_variables = BTreeSet::new();
                for member in members {
                    free_variables.extend(self.free_type_variables_in_core_type(&member)?);
                }
                Ok(free_variables)
            }
            CoreType::Variable(variable) => Ok(BTreeSet::from([variable])),
            CoreType::Nominal(_, type_arguments) => {
                let mut free_variables = BTreeSet::new();
                for type_argument in type_arguments {
                    free_variables.extend(self.free_type_variables_in_core_type(&type_argument)?);
                }
                Ok(free_variables)
            }
            CoreType::List(item_type) => self.free_type_variables_in_core_type(&item_type),
            CoreType::NamedList(item_type) => self.free_type_variables_in_core_type(&item_type),
            CoreType::Record(fields) => {
                let mut free_variables = BTreeSet::new();
                for field in fields {
                    free_variables.extend(self.free_type_variables_in_core_type(&field.value)?);
                }
                Ok(free_variables)
            }
            CoreType::Tuple(items) => {
                let mut free_variables = BTreeSet::new();
                for item in items {
                    free_variables.extend(self.free_type_variables_in_core_type(&item)?);
                }
                Ok(free_variables)
            }
            CoreType::Function(function_type) => {
                let mut free_variables = BTreeSet::new();

                for parameter in function_type.parameters {
                    free_variables.extend(self.free_type_variables_in_core_type(&parameter)?);
                }

                for named_parameter in function_type.named_parameters {
                    free_variables
                        .extend(self.free_type_variables_in_core_type(&named_parameter.value)?);
                }

                if let Some(element) = &function_type.variadic {
                    free_variables.extend(self.free_type_variables_in_core_type(element)?);
                }

                free_variables
                    .extend(self.free_type_variables_in_core_type(&function_type.return_type)?);

                Ok(free_variables)
            }
        }
    }

    fn resolve_function_type(
        &mut self,
        function_type: FunctionType<CoreType>,
    ) -> Result<FunctionType<CoreType>, InferenceError> {
        let mut resolved_parameters = Vec::with_capacity(function_type.parameters.len());
        for parameter in function_type.parameters {
            resolved_parameters.push(self.resolve(parameter)?);
        }

        let mut resolved_named_parameters =
            Vec::with_capacity(function_type.named_parameters.len());
        for named_parameter in function_type.named_parameters {
            resolved_named_parameters.push(RecordField::with_optional(
                named_parameter.name,
                self.resolve(named_parameter.value)?,
                named_parameter.optional,
            ));
        }

        let resolved_variadic = match function_type.variadic {
            Some(element) => Some(self.resolve(*element)?),
            None => None,
        };

        let resolved_return_type = self.resolve(*function_type.return_type)?;

        Ok(FunctionType::with_variadic(
            resolved_parameters,
            resolved_named_parameters,
            resolved_variadic,
            resolved_return_type,
        ))
    }

    fn unify_tuples(
        &mut self,
        left_items: Vec<CoreType>,
        right_items: Vec<CoreType>,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        if left_items.len() != right_items.len() {
            return Err(InferenceError::TupleLengthMismatch {
                expected: left_items.len(),
                actual: right_items.len(),
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            });
        }

        let mut unified_items = Vec::with_capacity(left_items.len());
        for (left_item, right_item) in left_items.into_iter().zip(right_items) {
            unified_items.push(self.unify_internal(left_item, right_item, expression)?);
        }

        Ok(CoreType::Tuple(unified_items))
    }

    fn unify_records(
        &mut self,
        left_fields: Vec<RecordField<CoreType>>,
        right_fields: Vec<RecordField<CoreType>>,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        let left_names: BTreeSet<_> = left_fields.iter().map(|field| field.name).collect();
        let right_names: BTreeSet<_> = right_fields.iter().map(|field| field.name).collect();

        if left_names != right_names {
            return Err(InferenceError::RecordFieldMismatch {
                expected_fields: left_names.into_iter().collect(),
                actual_fields: right_names.into_iter().collect(),
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            });
        }

        let right_by_name: BTreeMap<_, _> = right_fields
            .into_iter()
            .map(|field| (field.name, field.value))
            .collect();

        let mut unified_fields = Vec::with_capacity(left_fields.len());
        for left_field in left_fields {
            let Some(right_value) = right_by_name.get(&left_field.name).cloned() else {
                return Err(InferenceError::RecordFieldMismatch {
                    expected_fields: vec![left_field.name],
                    actual_fields: Vec::new(),
                    range: expression.map(|current_expression| current_expression.range),
                    expression_id: expression.map(|current_expression| current_expression.id),
                });
            };

            let unified_value = self.unify_internal(left_field.value, right_value, expression)?;
            unified_fields.push(RecordField::new(left_field.name, unified_value));
        }

        Ok(CoreType::Record(unified_fields))
    }

    // Parameters unify positionally across the flattened positional-then-named parameter
    // list: parameter names describe the call interface, not the identity of the function
    // type, so `fn(integer) -> NULL` and `fn(count: integer) -> NULL` unify. The left
    // function's interface (names and positional split) is kept for the result.
    fn unify_functions(
        &mut self,
        left_function: FunctionType<CoreType>,
        right_function: FunctionType<CoreType>,
        expression: Option<&Expression>,
    ) -> Result<FunctionType<CoreType>, InferenceError> {
        let left_total = left_function.parameters.len() + left_function.named_parameters.len();
        let right_total = right_function.parameters.len() + right_function.named_parameters.len();
        // A variadic function accepts a caller shape a fixed function does not, so the two are never the
        // same type. Treat a variadic/fixed mismatch as an arity mismatch (the rest parameter counts as
        // one interface slot the other side lacks).
        if left_total != right_total
            || left_function.variadic.is_some() != right_function.variadic.is_some()
        {
            return Err(InferenceError::FunctionArityMismatch {
                expected: left_total + usize::from(left_function.variadic.is_some()),
                actual: right_total + usize::from(right_function.variadic.is_some()),
                range: expression.map(|current_expression| current_expression.range),
                expression_id: expression.map(|current_expression| current_expression.id),
            });
        }

        let mut right_parameter_types = right_function.parameters;
        right_parameter_types.extend(
            right_function
                .named_parameters
                .into_iter()
                .map(|parameter| parameter.value),
        );
        let mut right_parameter_iter = right_parameter_types.into_iter();

        let mut unified_parameters = Vec::with_capacity(left_function.parameters.len());
        for left_parameter in left_function.parameters {
            let right_parameter = right_parameter_iter
                .next()
                .expect("parameter totals were checked to match");
            unified_parameters.push(self.unify_internal(
                left_parameter,
                right_parameter,
                expression,
            )?);
        }

        let mut unified_named_parameters = Vec::with_capacity(left_function.named_parameters.len());
        for left_named_parameter in left_function.named_parameters {
            let right_parameter = right_parameter_iter
                .next()
                .expect("parameter totals were checked to match");
            let unified_value =
                self.unify_internal(left_named_parameter.value, right_parameter, expression)?;
            unified_named_parameters.push(RecordField::with_optional(
                left_named_parameter.name,
                unified_value,
                left_named_parameter.optional,
            ));
        }

        let unified_variadic = match (left_function.variadic, right_function.variadic) {
            (Some(left_element), Some(right_element)) => {
                Some(self.unify_internal(*left_element, *right_element, expression)?)
            }
            // Presence was checked to match above, so only the both-absent case remains here.
            _ => None,
        };

        let unified_return_type = self.unify_internal(
            *left_function.return_type,
            *right_function.return_type,
            expression,
        )?;

        Ok(FunctionType::with_variadic(
            unified_parameters,
            unified_named_parameters,
            unified_variadic,
            unified_return_type,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperandShape {
    Scalar,
    Vector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumericResultAtomic {
    Promote,
    AlwaysDouble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComparisonFamily {
    Numeric,
    Character,
    Logical,
}

// The per-pair result of an arithmetic operator over the member shapes of its two operands: a
// vector member makes the pair's result a vector, both-`integer` pairs stay `integer`, and any
// `double` (or an always-`double` operator like `/`) promotes the pair. The caller joins the pairs
// into the operation's result, so `(integer | double) + integer` is `integer | double`.
fn member_wise_numeric_results(
    left_parts: &[(OperandShape, Atomic)],
    right_parts: &[(OperandShape, Atomic)],
    numeric_result_atomic: NumericResultAtomic,
) -> Vec<CoreType> {
    let mut results = Vec::with_capacity(left_parts.len() * right_parts.len());
    for (left_shape, left_atomic) in left_parts {
        for (right_shape, right_atomic) in right_parts {
            let shape =
                if *left_shape == OperandShape::Vector || *right_shape == OperandShape::Vector {
                    OperandShape::Vector
                } else {
                    OperandShape::Scalar
                };
            let atomic = match numeric_result_atomic {
                NumericResultAtomic::AlwaysDouble => Atomic::Double,
                NumericResultAtomic::Promote => {
                    if *left_atomic == Atomic::Integer && *right_atomic == Atomic::Integer {
                        Atomic::Integer
                    } else {
                        Atomic::Double
                    }
                }
            };
            results.push(core_type_for_shape(shape, atomic));
        }
    }
    results
}

// A comparison operand's member shapes: one for a concrete operand, all of them for a union
// (member-wise acceptance: every member must be comparable). `None` when any member is not.
fn comparison_operand_parts_list(
    core_type: &CoreType,
) -> Option<Vec<(OperandShape, ComparisonFamily)>> {
    match core_type {
        CoreType::Union(members) => members.iter().map(comparison_operand_parts).collect(),
        other => comparison_operand_parts(other).map(|parts| vec![parts]),
    }
}

// The shapes a comparison operand can take; a still-flexible bare variable behaves as a scalar
// (matching the pre-union result rule), while a flexible-element vector is known to be a vector.
fn shapes_for_operand(
    parts: &Option<Vec<(OperandShape, ComparisonFamily)>>,
    flexible: &Option<FlexibleComparisonOperand>,
) -> Vec<OperandShape> {
    match (parts, flexible) {
        (Some(parts), _) => parts.iter().map(|(shape, _)| *shape).collect(),
        (None, Some(FlexibleComparisonOperand::VectorElement(_))) => vec![OperandShape::Vector],
        _ => vec![OperandShape::Scalar],
    }
}

// A comparison operand whose family is not yet known: a bare inference variable, or a vector whose
// element is a still-generic variable (carried, so a numeric partner can constrain it) or is
// statically untracked (`Any`/`Unknown` element, nothing to constrain).
enum FlexibleComparisonOperand {
    Bare(InferenceVariableId),
    VectorElement(Option<InferenceVariableId>),
}

impl FlexibleComparisonOperand {
    fn variable(&self) -> Option<InferenceVariableId> {
        match self {
            FlexibleComparisonOperand::Bare(variable) => Some(*variable),
            FlexibleComparisonOperand::VectorElement(variable) => *variable,
        }
    }
}

fn flexible_comparison_operand(core_type: &CoreType) -> Option<FlexibleComparisonOperand> {
    match core_type {
        CoreType::Variable(variable) => Some(FlexibleComparisonOperand::Bare(*variable)),
        CoreType::Vector(element) | CoreType::NamedVector(element) => match element.as_ref() {
            CoreType::Variable(variable) => {
                Some(FlexibleComparisonOperand::VectorElement(Some(*variable)))
            }
            CoreType::Any | CoreType::Unknown => {
                Some(FlexibleComparisonOperand::VectorElement(None))
            }
            _ => None,
        },
        _ => None,
    }
}

fn comparison_operand_parts(core_type: &CoreType) -> Option<(OperandShape, ComparisonFamily)> {
    let (shape, atomic) = match core_type {
        CoreType::Scalar(atomic) => (OperandShape::Scalar, *atomic),
        CoreType::Vector(element) | CoreType::NamedVector(element) => {
            (OperandShape::Vector, element.element_atomic()?)
        }
        _ => return None,
    };

    let family = match atomic {
        Atomic::Integer | Atomic::Double => ComparisonFamily::Numeric,
        Atomic::Character => ComparisonFamily::Character,
        Atomic::Logical => ComparisonFamily::Logical,
        Atomic::Complex | Atomic::Raw => return None,
    };

    Some((shape, family))
}

// How an operand of an arithmetic operator classifies: a concrete numeric shape, a union whose
// members are all concrete numeric shapes (accepted member-wise), a still-flexible inference
// variable (which becomes numeric-constrained), an `Any`/`Unknown` short-circuit, or a hard error.
#[derive(Debug, Clone)]
enum NumericOperand {
    Concrete(OperandShape, Atomic),
    // Every member of a union operand, in member order. The operation applies to each member and
    // the result is the join of the per-member results (see "Operators over union operands" in the
    // typing reference).
    ConcreteUnion(Vec<(OperandShape, Atomic)>),
    Variable(InferenceVariableId),
    // A vector whose element is not yet concrete: a generic element variable (`T[]`, carrying the
    // variable to constrain) or a statically untracked element (`Any`/`Unknown`, carrying `None`).
    // The shape is known — vector — even though the atomic is not.
    FlexibleVector(Option<InferenceVariableId>),
    AnyUnknown,
    Invalid,
}

impl NumericOperand {
    // The operand's member shapes: one for a concrete operand, all of them for a union.
    fn concrete_parts(&self) -> Option<Vec<(OperandShape, Atomic)>> {
        match self {
            NumericOperand::Concrete(shape, atomic) => Some(vec![(*shape, *atomic)]),
            NumericOperand::ConcreteUnion(parts) => Some(parts.clone()),
            _ => None,
        }
    }
}

fn classify_numeric_operand(core_type: &CoreType) -> NumericOperand {
    if let Some((shape, atomic)) = numeric_operand_parts(core_type) {
        return NumericOperand::Concrete(shape, atomic);
    }
    match core_type {
        // A union operand is numeric when every member is: `Any`/`Unknown`/nested unions cannot
        // appear as members (union normalization absorbs or flattens them) and inference variables
        // cannot either (a join binds a variable rather than uniting over it), so any non-numeric
        // member makes the whole operand invalid — the error then shows the full union type.
        CoreType::Union(members) => {
            let mut parts = Vec::with_capacity(members.len());
            for member in members {
                match numeric_operand_parts(member) {
                    Some(part) => parts.push(part),
                    None => return NumericOperand::Invalid,
                }
            }
            NumericOperand::ConcreteUnion(parts)
        }
        CoreType::Variable(variable) => NumericOperand::Variable(*variable),
        CoreType::Vector(element) | CoreType::NamedVector(element) => match element.as_ref() {
            CoreType::Variable(variable) => NumericOperand::FlexibleVector(Some(*variable)),
            CoreType::Any | CoreType::Unknown => NumericOperand::FlexibleVector(None),
            _ => NumericOperand::Invalid,
        },
        CoreType::Any | CoreType::Unknown => NumericOperand::AnyUnknown,
        _ => NumericOperand::Invalid,
    }
}

fn constraint_is_satisfied(constraint: Constraint, core_type: &CoreType) -> bool {
    match constraint {
        Constraint::Unconstrained => true,
        Constraint::Numeric => is_numeric_core_type(core_type),
        Constraint::AtomicElement => matches!(core_type, CoreType::Scalar(_)),
        Constraint::ScalarNumeric => matches!(
            core_type,
            CoreType::Scalar(Atomic::Integer | Atomic::Double)
        ),
    }
}

fn is_numeric_core_type(core_type: &CoreType) -> bool {
    match core_type {
        CoreType::Scalar(Atomic::Integer | Atomic::Double) => true,
        CoreType::Vector(element) | CoreType::NamedVector(element) => matches!(
            element.as_ref(),
            CoreType::Scalar(Atomic::Integer | Atomic::Double)
        ),
        _ => false,
    }
}

fn constraint_violation_error(
    constraint: Constraint,
    actual: CoreType,
    expression: Option<&Expression>,
) -> InferenceError {
    InferenceError::ConstraintViolation {
        constraint,
        actual: Box::new(actual),
        range: expression.map(|expression| expression.range),
        expression_id: expression.map(|expression| expression.id),
    }
}

fn numeric_operand_parts(core_type: &CoreType) -> Option<(OperandShape, Atomic)> {
    match core_type {
        CoreType::Scalar(atomic @ (Atomic::Integer | Atomic::Double)) => {
            Some((OperandShape::Scalar, *atomic))
        }
        CoreType::Vector(element) | CoreType::NamedVector(element) => {
            match element.element_atomic()? {
                atomic @ (Atomic::Integer | Atomic::Double) => Some((OperandShape::Vector, atomic)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn combine_operand_atomic(core_type: &CoreType) -> Option<Atomic> {
    match core_type {
        CoreType::Scalar(atomic) => Some(*atomic),
        CoreType::Vector(element) | CoreType::NamedVector(element) => element.element_atomic(),
        _ => None,
    }
}

// `c(...)` follows R's atomic coercion hierarchy: logical < integer < double < complex < character,
// so mixed arguments promote to the widest type (this is what lets `c(1L, NA)` be `integer` and
// `c(1L, "a")` be `character`). `raw` does not participate and only combines with itself.
fn promote_combine_atomic(left: Atomic, right: Atomic) -> Option<Atomic> {
    if left == right {
        return Some(left);
    }
    let left_rank = combine_atomic_rank(left)?;
    let right_rank = combine_atomic_rank(right)?;
    Some(match left_rank.max(right_rank) {
        0 => Atomic::Logical,
        1 => Atomic::Integer,
        2 => Atomic::Double,
        3 => Atomic::Complex,
        _ => Atomic::Character,
    })
}

fn combine_atomic_rank(atomic: Atomic) -> Option<u8> {
    match atomic {
        Atomic::Logical => Some(0),
        Atomic::Integer => Some(1),
        Atomic::Double => Some(2),
        Atomic::Complex => Some(3),
        Atomic::Character => Some(4),
        Atomic::Raw => None,
    }
}

fn core_type_for_shape(shape: OperandShape, atomic: Atomic) -> CoreType {
    match shape {
        OperandShape::Scalar => CoreType::Scalar(atomic),
        OperandShape::Vector => CoreType::vector(atomic),
    }
}

// The (true-edge, false-edge) entry types a guard predicate induces over `entry`, `None` per edge
// when that edge leaves the entry untouched (and `None` overall when neither edge changes).
// Narrowing filters union members; a member whose family cannot be decided statically stays on
// both edges. The one non-union case: `is.null` on `Any`/`Unknown` pins the true edge to `NULL` —
// family guards deliberately do not invent a concrete shape for `Any`/`Unknown` (a refined union
// would false-positive against scalar-claim standard-library signatures).
fn refine_guarded_type(
    entry: &CoreType,
    predicate: GuardPredicate,
) -> Option<(Option<CoreType>, Option<CoreType>)> {
    match entry {
        CoreType::Union(members) => {
            let mut true_members = Vec::with_capacity(members.len());
            let mut false_members = Vec::with_capacity(members.len());
            let mut decided_any = false;
            for member in members {
                match member_matches_guard(member, predicate) {
                    Some(true) => {
                        true_members.push(member.clone());
                        decided_any = true;
                    }
                    Some(false) => {
                        false_members.push(member.clone());
                        decided_any = true;
                    }
                    None => {
                        true_members.push(member.clone());
                        false_members.push(member.clone());
                    }
                }
            }
            if !decided_any {
                return None;
            }
            // An edge with no members is impossible at runtime; dead branches are not typed
            // specially, so such an edge stays untouched. An edge keeping every member changes
            // nothing and stays untouched too.
            let true_type = (!true_members.is_empty() && true_members.len() < members.len())
                .then(|| CoreType::union_of(true_members));
            let false_type = (!false_members.is_empty() && false_members.len() < members.len())
                .then(|| CoreType::union_of(false_members));
            if true_type.is_none() && false_type.is_none() {
                return None;
            }
            Some((true_type, false_type))
        }
        CoreType::Any | CoreType::Unknown if predicate == GuardPredicate::Null => {
            Some((Some(CoreType::Null), None))
        }
        _ => None,
    }
}

// Whether one (normalized, non-union) member satisfies a guard predicate: `Some(bool)` when the
// member's runtime family is statically certain, `None` when it is not (inference variables,
// flexible-element vectors, opaque nominals — `is.list(data.frame)` is true at runtime, for
// example, which the checker cannot see through a sealed type).
fn member_matches_guard(member: &CoreType, predicate: GuardPredicate) -> Option<bool> {
    match member {
        // A nested union inside a normalized union cannot occur; treat it as undecidable if a
        // denormalized value ever reaches here rather than mis-filtering it.
        CoreType::Variable(_)
        | CoreType::Any
        | CoreType::Unknown
        | CoreType::Nominal(..)
        | CoreType::Union(_) => None,
        CoreType::Null => Some(predicate == GuardPredicate::Null),
        CoreType::Scalar(atomic) => Some(atomic_matches_guard(*atomic, predicate)),
        CoreType::Vector(element) | CoreType::NamedVector(element) => match element.as_ref() {
            CoreType::Scalar(atomic) => Some(atomic_matches_guard(*atomic, predicate)),
            _ => None,
        },
        CoreType::Function(_) => Some(predicate == GuardPredicate::Function),
        CoreType::List(_) | CoreType::NamedList(_) | CoreType::Tuple(_) | CoreType::Record(_) => {
            Some(predicate == GuardPredicate::List)
        }
    }
}

fn atomic_matches_guard(atomic: Atomic, predicate: GuardPredicate) -> bool {
    match predicate {
        GuardPredicate::Character => atomic == Atomic::Character,
        GuardPredicate::Logical => atomic == Atomic::Logical,
        GuardPredicate::Integer => atomic == Atomic::Integer,
        GuardPredicate::Double => atomic == Atomic::Double,
        GuardPredicate::Numeric => matches!(atomic, Atomic::Integer | Atomic::Double),
        GuardPredicate::Null | GuardPredicate::Function | GuardPredicate::List => false,
    }
}

fn nullable_type(core_type: CoreType) -> CoreType {
    CoreType::union_of(vec![core_type, CoreType::Null])
}

// The one atomic widening compatibility admits: `integer` fits where `double` is expected. All
// other atomic pairs must match exactly.
fn atomic_widens_to(actual: Atomic, expected: Atomic) -> bool {
    actual == expected || (actual == Atomic::Integer && expected == Atomic::Double)
}

// Replaces every inference variable in an (already-resolved) type with `Unknown`, for values that
// must survive a later unification rollback: a stored variable id would dangle once the rollback
// reclaims (and later reuses) the id.
fn erase_variables(core_type: CoreType) -> CoreType {
    match core_type {
        CoreType::Variable(_) => CoreType::Unknown,
        CoreType::Union(members) => {
            CoreType::union_of(members.into_iter().map(erase_variables).collect())
        }
        CoreType::List(inner) => CoreType::List(Box::new(erase_variables(*inner))),
        CoreType::NamedList(inner) => CoreType::NamedList(Box::new(erase_variables(*inner))),
        CoreType::Record(fields) => CoreType::Record(
            fields
                .into_iter()
                .map(|field| {
                    RecordField::with_optional(
                        field.name,
                        erase_variables(field.value),
                        field.optional,
                    )
                })
                .collect(),
        ),
        CoreType::Tuple(items) => CoreType::Tuple(items.into_iter().map(erase_variables).collect()),
        CoreType::Nominal(name, arguments) => {
            CoreType::Nominal(name, arguments.into_iter().map(erase_variables).collect())
        }
        CoreType::Function(function_type) => CoreType::Function(FunctionType {
            parameters: function_type
                .parameters
                .into_iter()
                .map(erase_variables)
                .collect(),
            named_parameters: function_type
                .named_parameters
                .into_iter()
                .map(|parameter| {
                    RecordField::with_optional(
                        parameter.name,
                        erase_variables(parameter.value),
                        parameter.optional,
                    )
                })
                .collect(),
            variadic: function_type
                .variadic
                .map(|element| Box::new(erase_variables(*element))),
            return_type: Box::new(erase_variables(*function_type.return_type)),
        }),
        CoreType::Vector(element) => CoreType::Vector(Box::new(erase_variables(*element))),
        CoreType::NamedVector(element) => {
            CoreType::NamedVector(Box::new(erase_variables(*element)))
        }
        other @ (CoreType::Any | CoreType::Unknown | CoreType::Null | CoreType::Scalar(_)) => other,
    }
}

// Rewrites an indexing error raised against one union member so it reports the full union — the
// subject's actual type. Only the container/actual payload changes; range and identity stay.
fn widen_error_container_to_union(error: InferenceError, union_type: &CoreType) -> InferenceError {
    match error {
        InferenceError::FieldDoesNotExist {
            field,
            range,
            expression_id,
            ..
        } => InferenceError::FieldDoesNotExist {
            field,
            container: Box::new(union_type.clone()),
            range,
            expression_id,
        },
        InferenceError::PositionDoesNotExist {
            position,
            range,
            expression_id,
            ..
        } => InferenceError::PositionDoesNotExist {
            position,
            container: Box::new(union_type.clone()),
            range,
            expression_id,
        },
        InferenceError::NonLiteralSubscript {
            by,
            range,
            expression_id,
            ..
        } => InferenceError::NonLiteralSubscript {
            container: Box::new(union_type.clone()),
            by,
            range,
            expression_id,
        },
        InferenceError::NotAList {
            range,
            expression_id,
            ..
        } => InferenceError::NotAList {
            actual: Box::new(union_type.clone()),
            range,
            expression_id,
        },
        InferenceError::UnsupportedSubset {
            range,
            expression_id,
            ..
        } => InferenceError::UnsupportedSubset {
            actual: Box::new(union_type.clone()),
            range,
            expression_id,
        },
        other => other,
    }
}

// The one member-wise union shape `unify` handles: a normalized two-member `T | NULL` union
// exposes its non-`NULL` member. Everything else returns `None`.
fn nullable_single_member(members: &[CoreType]) -> Option<CoreType> {
    match members {
        [member, CoreType::Null] if *member != CoreType::Null => Some(member.clone()),
        _ => None,
    }
}

fn iterable_item_type(core_type: &CoreType) -> Option<CoreType> {
    match core_type {
        CoreType::Scalar(atomic) => Some(CoreType::Scalar(*atomic)),
        CoreType::Vector(element) | CoreType::NamedVector(element) => Some((**element).clone()),
        CoreType::List(item_type) | CoreType::NamedList(item_type) => Some((**item_type).clone()),
        // A fixed-shape list iterates every element, so the loop variable can hold any of the item
        // types: the element type is their union (which collapses back to the single item type for
        // a homogeneous list; the empty list's element type is `NULL`, the union of zero members).
        CoreType::Tuple(items) => Some(CoreType::union_of(items.clone())),
        CoreType::Record(fields) => Some(CoreType::union_of(
            fields.iter().map(|field| field.value.clone()).collect(),
        )),
        // Member-wise over a union iterable: every member must itself be iterable, and the loop
        // variable can hold any member's element type.
        CoreType::Union(members) => {
            let mut item_types = Vec::with_capacity(members.len());
            for member in members {
                item_types.push(iterable_item_type(member)?);
            }
            Some(CoreType::union_of(item_types))
        }
        _ => None,
    }
}

fn import_core_type(
    core_type: &CoreType,
    substitutions: &BTreeMap<InferenceVariableId, InferenceVariableId>,
) -> CoreType {
    match core_type {
        CoreType::Any => CoreType::Any,
        CoreType::Unknown => CoreType::Unknown,
        CoreType::Null => CoreType::Null,
        // Re-normalize: importing can collapse members (a free variable erases to `Unknown`, which
        // absorbs the union) or make two members equal.
        CoreType::Union(members) => CoreType::union_of(
            members
                .iter()
                .map(|member| import_core_type(member, substitutions))
                .collect(),
        ),
        CoreType::Scalar(atomic) => CoreType::Scalar(*atomic),
        CoreType::Nominal(symbol, type_arguments) => CoreType::Nominal(
            *symbol,
            type_arguments
                .iter()
                .map(|type_argument| import_core_type(type_argument, substitutions))
                .collect(),
        ),
        CoreType::Vector(element) => {
            CoreType::Vector(Box::new(import_core_type(element, substitutions)))
        }
        CoreType::NamedVector(element) => {
            CoreType::NamedVector(Box::new(import_core_type(element, substitutions)))
        }
        CoreType::List(item_type) => {
            CoreType::List(Box::new(import_core_type(item_type, substitutions)))
        }
        CoreType::NamedList(item_type) => {
            CoreType::NamedList(Box::new(import_core_type(item_type, substitutions)))
        }
        CoreType::Record(fields) => CoreType::Record(
            fields
                .iter()
                .map(|field| {
                    RecordField::with_optional(
                        field.name,
                        import_core_type(&field.value, substitutions),
                        field.optional,
                    )
                })
                .collect(),
        ),
        CoreType::Tuple(items) => CoreType::Tuple(
            items
                .iter()
                .map(|item| import_core_type(item, substitutions))
                .collect(),
        ),
        CoreType::Function(function_type) => CoreType::Function(FunctionType::with_variadic(
            function_type
                .parameters
                .iter()
                .map(|parameter| import_core_type(parameter, substitutions))
                .collect(),
            function_type
                .named_parameters
                .iter()
                .map(|parameter| {
                    RecordField::with_optional(
                        parameter.name,
                        import_core_type(&parameter.value, substitutions),
                        parameter.optional,
                    )
                })
                .collect(),
            function_type
                .variadic
                .as_ref()
                .map(|element| import_core_type(element, substitutions)),
            import_core_type(&function_type.return_type, substitutions),
        )),
        // A variable absent from `substitutions` is a free scheme variable that belongs to another
        // document's `InferenceState`; it has no meaning here, so importing erases it to `Unknown`.
        // This is a deliberate erasure of a map lookup miss, not a discarded error.
        CoreType::Variable(variable) => substitutions
            .get(variable)
            .copied()
            .map(CoreType::Variable)
            .unwrap_or(CoreType::Unknown),
    }
}

fn is_whole_number_double_literal(expression: &Expression) -> bool {
    let ExpressionKind::Double(text) = &expression.kind else {
        return false;
    };
    text.parse::<f64>()
        .is_ok_and(|value| value.fract() == 0.0 && value.is_finite())
}

fn integer_literal_position(expression: &Expression) -> Option<usize> {
    // R indexes identically with `x[[2]]` and `x[[2L]]`, so a whole-number double literal is just as
    // valid a statically known position as an integer literal.
    let one_based_index = match &expression.kind {
        ExpressionKind::Integer(text) => text.trim_end_matches('L').parse::<usize>().ok()?,
        ExpressionKind::Double(text) => {
            let value = text.parse::<f64>().ok()?;
            if value.fract() != 0.0 || value < 1.0 || !value.is_finite() {
                return None;
            }
            value as usize
        }
        _ => return None,
    };
    one_based_index.checked_sub(1)
}

fn literal_name_symbol(expression: &Expression) -> Option<Symbol> {
    let ExpressionKind::StringLiteralName(symbol) = &expression.kind else {
        return None;
    };
    Some(*symbol)
}

fn alias_cycle_error(symbol: Symbol, expression: Option<&Expression>) -> InferenceError {
    let fallback_range = Range {
        start_byte: 0,
        end_byte: 0,
        start_point: tree_sitter::Point::new(0, 0),
        end_point: tree_sitter::Point::new(0, 0),
    };
    InferenceError::AliasCycle {
        symbol,
        range: expression
            .map(|expression| expression.range)
            .unwrap_or(fallback_range),
        expression_id: expression.map(|expression| expression.id),
    }
}

// Aligns an expected (annotated) function type's parameter types to the definition's formals.
// R call sites match arguments against the *definition's* formal names, so a named annotation
// parameter must bind to the same-named formal — a positional zip would type the body's formals
// against the wrong slots whenever the annotation and definition order them differently, and a
// by-name call would then route a value into a formal checked at a different type.
//
// Named annotation parameters bind by name; unnamed (positional) annotation parameters fill the
// remaining formals left to right. `Ok(None)` means the shapes cannot align for a reason ordinary
// function compatibility will diagnose (an arity mismatch); a named parameter that matches no
// formal is its own hard error, because the annotation would otherwise promise callers a name the
// runtime rejects.
// The overloadable name a call's callee spells, when it is a bare or namespace-qualified name
// that does NOT resolve to a local slot or package global (an overload set never shadows user
// code — only base-environment stub names participate).
fn callee_overload_symbol(
    callee: &Expression,
    resolution_context: Option<&ResolutionContext<'_>>,
) -> Option<Symbol> {
    match &callee.kind {
        ExpressionKind::Symbol(symbol) => {
            if let Some(context) = resolution_context {
                if context
                    .local_naming
                    .expression_resolutions
                    .contains_key(&callee.id)
                {
                    return None;
                }
                if context.package_naming.global_bindings.contains_key(symbol) {
                    return None;
                }
            }
            Some(*symbol)
        }
        ExpressionKind::NamespaceGet { name, .. } => Some(*name),
        _ => None,
    }
}

fn align_expected_parameter_types(
    function_type: &FunctionType<CoreType>,
    parameters: &[crate::hir::Parameter],
    annotation_range: Range,
) -> Result<Option<Vec<CoreType>>, InferenceError> {
    if function_type.parameters.len() + function_type.named_parameters.len() != parameters.len() {
        return Ok(None);
    }
    let mut aligned: Vec<Option<CoreType>> = vec![None; parameters.len()];
    for named_parameter in &function_type.named_parameters {
        let Some(index) = parameters
            .iter()
            .position(|parameter| parameter.symbol == named_parameter.name)
        else {
            return Err(InferenceError::AnnotationParameterNameMismatch {
                name: named_parameter.name,
                range: Some(annotation_range),
            });
        };
        if aligned[index].is_some() {
            return Ok(None);
        }
        aligned[index] = Some(named_parameter.value.clone());
    }
    let mut positional = function_type.parameters.iter();
    for slot in aligned.iter_mut() {
        if slot.is_none() {
            *slot = positional.next().cloned();
        }
    }
    Ok(aligned.into_iter().collect())
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
