use {
    crate::{
        document::DocumentId,
        hir::{
            AssignTarget, AssignmentScope, DefinitionKind, Expression, ExpressionId,
            ExpressionKind, HirArena, Module, contains_loop_exit, replacement_base,
        },
        interner::{Interner, Symbol},
        lower::LoweringContext,
        naming::{BindingId, NamesGlobal, NamesLocal, find_binding},
        types::{
            Annotation, Atomic, Constraint, CoreType, FunctionType, InferenceVariableId,
            RecordField, RestParameter, SurfaceType, TypeScheme,
        },
    },
    rustc_hash::FxHashMap,
    std::collections::{BTreeMap, BTreeSet},
    tree_sitter::Range,
};

mod annotations;
mod calls;
mod control;
mod environment;
mod operand;
mod operators;
mod unify;
use control::{GUARD_PREDICATES, GuardPredicate};
use operand::{alias_cycle_error, align_expected_parameter_types, erase_variables};

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
    // Shared, not owned: the stub-laden template environment is cloned per whole-file inference,
    // and sharing the schemes turns that clone into refcount bumps instead of deep copies of
    // hundreds of stub signatures.
    pub type_scheme: std::sync::Arc<TypeScheme>,
    pub range: Range,
    // The slot is a defaultless parameter on a control-flow edge where `missing(name)` held, so a
    // read would fail at run time (R: "argument is missing, with no default"). Set only by the
    // missing()-guard refinement; any write to the slot clears it.
    pub unsupplied: bool,
}

impl Binding {
    pub fn new(type_scheme: TypeScheme, range: Range) -> Binding {
        Binding {
            type_scheme: std::sync::Arc::new(type_scheme),
            range,
            unsupplied: false,
        }
    }

    pub fn monomorphic(core_type: CoreType, range: Range) -> Binding {
        Binding::new(TypeScheme::monomorphic(core_type), range)
    }
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
    UnsupportedSubset {
        actual: Box<CoreType>,
        range: Range,
        expression_id: ExpressionId,
    },
    RecursionLimitExceeded,
}

// The union-find entry table. Variable ids are allocated densely from zero and reclaimed only by
// truncating the tail (probe rollback), so the table is a plain vector indexed by id: entry lookup
// is the hottest operation in inference, and a tree search per resolve step dominated whole-file
// checks on large files. The map-shaped accessors keep call sites identical to the map they
// replaced; the id counter is the length, so a dangling id is unrepresentable.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntryTable(Vec<InferenceEntry>);

impl EntryTable {
    pub(crate) fn get(&self, variable: &InferenceVariableId) -> Option<&InferenceEntry> {
        self.0.get(variable.0 as usize)
    }

    pub(crate) fn contains_key(&self, variable: &InferenceVariableId) -> bool {
        (variable.0 as usize) < self.0.len()
    }

    pub(crate) fn insert(&mut self, variable: InferenceVariableId, entry: InferenceEntry) {
        let index = variable.0 as usize;
        match index.cmp(&self.0.len()) {
            std::cmp::Ordering::Less => self.0[index] = entry,
            std::cmp::Ordering::Equal => self.0.push(entry),
            // A gap would mean an id was minted without allocating its entry — corrupted
            // inference state, unrecoverable.
            std::cmp::Ordering::Greater => {
                panic!("inference variable ids must be allocated densely")
            }
        }
    }

    pub(crate) fn truncate(&mut self, count: u32) {
        self.0.truncate(count as usize);
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct InferenceState {
    current_level: Level,
    entries: EntryTable,
    // Hash-keyed: the hottest read path (every name lookup) and never iterated, so no order is
    // observable.
    environment: FxHashMap<EnvironmentKey, Binding>,
    builtins: BTreeMap<Symbol, BuiltinKind>,
    // When enabled, every inferred expression's result type is recorded by id so tooling (hover,
    // inlay hints) can show checked types. Left off during interface rounds to avoid the cost.
    record_expression_types: bool,
    recorded_expression_types: FxHashMap<ExpressionId, CoreType>,
    // For each call whose callee resolved to a stub overload set, the index (into the declared set)
    // of the scheme the call committed, keyed by the callee expression. Only the selection pass
    // knows which candidate won, and signature help needs it to mark the active signature; recorded
    // on the commit path only, so failed probes leave nothing behind.
    selected_overloads: FxHashMap<ExpressionId, usize>,
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
    // Arc'd for the same template-clone reason as `Binding.type_scheme`: the sets are read-only
    // after seeding, and the per-inference clone must not deep-copy every overloaded signature.
    overload_sets: BTreeMap<Symbol, std::sync::Arc<Vec<TypeScheme>>>,
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
    // The whole-file letrec members: each top-level function-valued assignment target, pre-bound
    // before any statement of the module is inferred (see `check_module_with_naming`), keyed by
    // symbol so the assignment itself reuses the variable its earlier readers constrained. Members
    // stay monomorphic until the whole module is inferred (mutual members constrain each other
    // across statements), then one finalization pass defaults + generalizes each. Cleared per
    // module.
    module_letrec_placeholders: BTreeMap<Symbol, ModuleLetrecMember>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    top_level_expression_ids: &'a BTreeSet<ExpressionId>,
    // Every symbol's exported binding, precomputed once per check: the winner test runs per
    // top-level assignment, and a per-assignment `find_exported_binding` scan was quadratic in
    // the statement count.
    exported_bindings: &'a BTreeMap<Symbol, BindingId>,
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

    /// Overlay one module's declarations on top of an existing environment: each declared name
    /// wins over whatever the environment held (a package definition or a seeded stub type),
    /// exactly as a later module wins in [`from_modules`](Self::from_modules) and a module
    /// declaration wins over seeding. This is how a script's own declarations shadow the
    /// package-wide environment without rebuilding it from every package module.
    pub fn extend_from_module(&mut self, module: &Module) {
        for definition in &module.definitions {
            self.definitions.insert(
                definition.definition.name,
                TypeDefinition {
                    kind: definition.definition.kind,
                    type_parameters: definition.definition.type_parameters.clone(),
                    representation: Some(definition.definition.surface_type.clone()),
                },
            );
        }
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
        SurfaceType::Any
        | SurfaceType::Unknown
        | SurfaceType::Null
        | SurfaceType::ElidedReturn
        | SurfaceType::Scalar(_) => {}
    }
}

// One whole-file letrec member: the shared inference variable, the top-level expression that is
// its (last-writer) definition, the local slot the definition also binds, and the definition range.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleLetrecMember {
    variable: InferenceVariableId,
    defining_expression: ExpressionId,
    binding_id: Option<BindingId>,
    range: Range,
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
        let top_level_expression_ids: BTreeSet<ExpressionId> =
            module.expressions.iter().copied().collect();
        let mut exported_bindings = BTreeMap::new();
        collect_exported_bindings(
            module,
            local_naming,
            &module.expressions,
            &mut exported_bindings,
        );
        let resolution_context = ResolutionContext {
            document_id,
            top_level_expression_ids: &top_level_expression_ids,
            exported_bindings: &exported_bindings,
            local_naming,
            package_naming,
        };
        // Loop memos and captured-write joins are keyed by per-module expression/binding ids;
        // clear them so a state reused across documents cannot alias.
        self.loop_memos.clear();
        self.captured_write_joins.clear();

        self.bind_module_letrec_placeholders(module);

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

        errors.extend(self.finalize_module_letrec_members());

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
        // One reusable buffer across the sweep: this visits every recorded expression type, so a
        // per-type set allocation (let alone resolving a deep copy of each type) dominated whole-file
        // check time on large files.
        let mut variable_constraints = BTreeMap::new();
        let mut free_variables: Vec<InferenceVariableId> = Vec::new();
        for core_type in expression_types_by_id
            .values()
            .chain(expression_types.iter())
        {
            free_variables.clear();
            let visited = self.visit_unbound_variables(core_type, 0, &mut |variable| {
                free_variables.push(variable);
            });
            if visited.is_err() {
                continue;
            }
            for variable in &free_variables {
                if let Some(InferenceEntry::Unbound { constraint, .. }) = self.entries.get(variable)
                    && *constraint != Constraint::Unconstrained
                {
                    variable_constraints.insert(*variable, *constraint);
                }
            }
        }
        ModuleCheck {
            expression_types,
            expression_types_by_id,
            variable_constraints,
            selected_overloads: std::mem::take(&mut self.selected_overloads)
                .into_iter()
                .collect(),
            errors,
            strict_origins,
        }
    }

    /// Infers ONE top-level definition statement against an environment of imported schemes and
    /// returns the defined symbol's scheme — the per-definition unit the engine's interface fixed
    /// point re-infers for files that decompose per [`scc_definition_plan`]. `imports` carries a
    /// scheme for every package global the statement references (see
    /// [`statement_reference_symbols`]); under the plan's conditions those interface values equal
    /// the whole-file walk's values at the reads, so the resulting scheme matches the whole-file
    /// inference up to inference variable identity. The whole check runs inside a snapshot and
    /// rolls back completely, so one state serves every definition of a fixed point — a fresh
    /// stub-seeded state per definition dominated the rounds — and each definition's variable ids
    /// stay deterministic regardless of what ran before it.
    #[allow(clippy::too_many_arguments)]
    pub fn check_definition_scheme(
        &mut self,
        document_id: DocumentId,
        module: &Module,
        statement: ExpressionId,
        local_naming: &NamesLocal,
        package_naming: &NamesGlobal,
        type_definitions: &TypeDefinitionEnvironment,
        imports: &[(Symbol, TypeScheme)],
    ) -> Option<TypeScheme> {
        let top_level_expression_ids: BTreeSet<ExpressionId> =
            module.expressions.iter().copied().collect();
        let mut exported_bindings = BTreeMap::new();
        collect_exported_bindings(
            module,
            local_naming,
            &module.expressions,
            &mut exported_bindings,
        );
        let resolution_context = ResolutionContext {
            document_id,
            top_level_expression_ids: &top_level_expression_ids,
            exported_bindings: &exported_bindings,
            local_naming,
            package_naming,
        };
        self.loop_memos.clear();
        self.captured_write_joins.clear();
        self.pending_enclosing_writes.clear();
        let environment_snapshot = self.environment_snapshot();
        let unification_snapshot = self.snapshot();
        for (symbol, scheme) in imports {
            let imported = self.import_scheme(scheme);
            // The synthetic range mirrors `infer_file`'s import binding: the range is read only by
            // export extraction, never by a diagnostic.
            self.bind_global_scheme(
                *symbol,
                imported,
                Range {
                    start_byte: 0,
                    end_byte: 0,
                    start_point: tree_sitter::Point { row: 0, column: 0 },
                    end_point: tree_sitter::Point { row: 0, column: 0 },
                },
            );
        }
        // Mirror the whole-file walk's level bookkeeping: the letrec pre-pass always enters a
        // level (the plan guarantees zero members, so no placeholders bind) and finalization exits
        // it before the exports are read.
        self.enter_level();
        self.module_letrec_placeholders.clear();
        let expression = module.arena.get(statement);
        let outcome = self.infer_expression_with_context(
            expression,
            &module.arena,
            Some(&resolution_context),
            type_definitions,
        );
        if outcome.is_err() {
            // The whole-file walk's recovery: a failed top-level binding resolves to `Unknown`
            // through both lookup paths, and that recovery binding is what the export reads.
            let recovery_scheme = TypeScheme::monomorphic(CoreType::Unknown);
            if let Some(binding_id) = local_naming.expression_resolutions.get(&statement).copied() {
                self.bind_local_scheme(binding_id, recovery_scheme.clone(), expression.range);
            }
            if let Some(target) = expression.kind.assignment_variable()
                && package_naming.global_bindings.contains_key(&target)
            {
                self.bind_global_scheme(target, recovery_scheme, expression.range);
            }
        }
        self.pending_enclosing_writes.clear();
        self.exit_level();
        let scheme = expression
            .kind
            .simple_assignment_target()
            .and_then(|symbol| {
                let binding_id = exported_bindings.get(&symbol).copied()?;
                let binding = self
                    .lookup_local_name(binding_id)
                    .or_else(|| self.lookup_global_name(symbol))?;
                Some((*binding.type_scheme).clone())
            });
        self.environment_rollback(environment_snapshot);
        self.rollback_to(unification_snapshot);
        scheme
    }

    pub fn exported_value_schemes(
        &self,
        module: &Module,
        local_naming: &NamesLocal,
    ) -> Vec<ExportedValue> {
        let mut symbols_in_order = Vec::new();
        let mut seen = BTreeSet::new();
        for expression_id in &module.expressions {
            if let Some(target) = module
                .arena
                .get(*expression_id)
                .kind
                .simple_assignment_target()
                && seen.insert(target)
            {
                symbols_in_order.push(target);
            }
        }

        // One walk collects every symbol's exported binding (its last *resolved* assignment,
        // recursing into bare blocks), replacing a per-symbol reverse scan that made files with
        // many exports quadratic. An unresolved assignment never overwrites: the reverse scan it
        // replaces skipped those and kept looking earlier.
        let mut exported_bindings: BTreeMap<Symbol, BindingId> = BTreeMap::new();
        collect_exported_bindings(
            module,
            local_naming,
            &module.expressions,
            &mut exported_bindings,
        );

        symbols_in_order
            .into_iter()
            .filter_map(|symbol| {
                let binding_id = exported_bindings.get(&symbol).copied()?;
                let binding = self
                    .lookup_local_name(binding_id)
                    .or_else(|| self.lookup_global_name(symbol))?;
                Some(ExportedValue {
                    symbol,
                    type_scheme: (*binding.type_scheme).clone(),
                    range: binding.range,
                })
            })
            .collect()
    }

    // Whole-file letrec: every top-level function-valued assignment target is pre-bound to a
    // fresh monomorphic variable before any statement is inferred, so recursion in all its
    // top-level forms — self-reference, mutual reference, a forward reference from an earlier
    // body to a later definition — unifies with the eventually-inferred function type instead
    // of importing the interface fixed point's Unknown-pinned scheme. The per-assignment
    // letrec placeholder reuses these variables (`module_letrec_placeholders`), which is what
    // joins an earlier body's constraints with the definition's final type. R semantics: a
    // closure body only runs after the whole file has loaded, so every top-level binding is
    // in scope by then.
    fn bind_module_letrec_placeholders(&mut self, module: &Module) {
        let candidates = module_letrec_candidates(module);
        let mutual_members = module_letrec_member_symbols_from(module, &candidates);

        // Members live one level below the module scope: unification level-adjusts every variable
        // the group's bodies share up to the placeholders' level, so the placeholders must sit
        // deeper than the level finalization generalizes at — otherwise a genuinely polymorphic
        // member exports a *free* variable, which a consuming document erases to `Unknown`.
        // `finalize_module_letrec_members` exits this level before generalizing.
        self.enter_level();
        self.module_letrec_placeholders.clear();
        for (symbol, (expression_id, _, range)) in &candidates {
            if !mutual_members.contains(symbol) {
                continue;
            }
            let variable = self.fresh_variable();
            self.module_letrec_placeholders.insert(
                *symbol,
                ModuleLetrecMember {
                    variable,
                    defining_expression: *expression_id,
                    binding_id: None,
                    range: *range,
                },
            );
            self.bind_global_name(*symbol, CoreType::Variable(variable), *range);
        }
    }

    // The module letrec finalization: with every statement inferred, the mutual group is fully
    // constrained — default the escaping numerics and generalize each member, rebinding the same
    // environment keys its definition wrote. Failures poison only their own member.
    fn finalize_module_letrec_members(&mut self) -> Vec<InferenceError> {
        self.exit_level();
        let mut errors = Vec::new();
        for (symbol, member) in std::mem::take(&mut self.module_letrec_placeholders) {
            let finalized = self
                .resolve(CoreType::Variable(member.variable))
                .and_then(|resolved| self.default_free_numeric(resolved))
                .and_then(|defaulted| self.generalize(defaulted));
            match finalized {
                Ok(generalized) => {
                    if let Some(binding_id) = member.binding_id {
                        self.bind_local_scheme(binding_id, generalized.clone(), member.range);
                    }
                    self.bind_global_scheme(symbol, generalized, member.range);
                }
                Err(error) => errors.push(error),
            }
        }
        errors
    }

    fn infer_module_with_context(
        &mut self,
        module: &Module,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<Vec<CoreType>, InferenceError> {
        self.bind_module_letrec_placeholders(module);
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
        if let Some(error) = self.finalize_module_letrec_members().into_iter().next() {
            return Err(error);
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
            // A parse hole: the syntax diagnostic already covers the region, so the checker draws
            // no conclusion from it — `Unknown` without even a strict origin.
            ExpressionKind::Missing => Ok(CoreType::Unknown),
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
        // Monomorphic recursion (letrec for closures): a function-valued RHS can read its own
        // name — the naming walk resolves that read to this very binding — so the target slot is
        // pre-bound to a fresh variable before the body is inferred, and the variable unifies
        // with the inferred function type afterwards. Recursive calls thereby constrain the
        // function's own parameters and return instead of typing as a silent unbound `Unknown`.
        // A top-level definition reuses the module letrec variable (see the whole-file pre-pass)
        // so mutual and forward references — inferred before this assignment ran — share the very
        // same variable this assignment unifies with its inferred type. Members stay monomorphic
        // here; the module finalization pass generalizes them together.
        let module_letrec_member = matches!(arena.get(value).kind, ExpressionKind::Function { .. })
            && self
                .module_letrec_placeholders
                .get(&target)
                .is_some_and(|member| member.defining_expression == expression.id);
        let recursion_placeholder =
            if matches!(arena.get(value).kind, ExpressionKind::Function { .. }) {
                let variable = if module_letrec_member {
                    self.module_letrec_placeholders[&target].variable
                } else {
                    self.fresh_variable()
                };
                let local_binding = resolution_context.and_then(|context| {
                    context
                        .local_naming
                        .expression_resolutions
                        .get(&expression.id)
                        .copied()
                });
                if module_letrec_member
                    && let Some(member) = self.module_letrec_placeholders.get_mut(&target)
                {
                    member.binding_id = local_binding;
                }
                match local_binding {
                    Some(binding_id) => self.bind_local_name(
                        binding_id,
                        CoreType::Variable(variable),
                        expression.range,
                    ),
                    None => self.bind_global_name(
                        target,
                        CoreType::Variable(variable),
                        expression.range,
                    ),
                }
                Some(variable)
            } else {
                None
            };
        let binding_type_result = self.infer_assign_binding_type(
            value,
            expression,
            arena,
            resolution_context,
            type_definitions,
        );
        let binding_type_result = match (recursion_placeholder, binding_type_result) {
            (Some(variable), Ok(inferred)) => {
                self.unify_with_context(CoreType::Variable(variable), inferred, expression)
            }
            (_, result) => result,
        };
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
                .zip(resolution_context.exported_bindings.get(&target))
                .is_some_and(|(binding_id, export_binding_id)| binding_id == export_binding_id)
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
                    let generalized_scheme = if module_letrec_member {
                        TypeScheme::monomorphic(binding_type.clone())
                    } else {
                        self.generalize(binding_type.clone())?
                    };
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
            let generalized_scheme = if module_letrec_member {
                TypeScheme::monomorphic(binding_type.clone())
            } else {
                self.generalize(binding_type.clone())?
            };
            // The recursion placeholder pre-bound the per-site local slot, and the export
            // extraction reads the local entry before the global one — keep them consistent.
            if let Some(binding_id) = resolution_context
                .local_naming
                .expression_resolutions
                .get(&expression.id)
                .copied()
            {
                self.bind_local_scheme(binding_id, generalized_scheme.clone(), expression.range);
            }
            self.bind_global_scheme(target, generalized_scheme, expression.range);
            return Ok(binding_type);
        }

        let generalized_scheme = if module_letrec_member {
            TypeScheme::monomorphic(binding_type.clone())
        } else {
            self.generalize(binding_type.clone())?
        };
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
        let value_type = self.resolve(value_type.clone())?;

        // A known-field write (`$field` / `[["literal"]]`) names a field statically.
        if let Some(field_name) = field_name {
            match prior_type {
                CoreType::Record(fields) => {
                    let mut updated_fields = fields.clone();
                    match updated_fields
                        .iter_mut()
                        .find(|field| field.name == field_name)
                    {
                        Some(field) => field.value = value_type,
                        None => updated_fields.push(RecordField::new(field_name, value_type)),
                    }
                    return Ok(CoreType::Record(updated_fields));
                }
                // A known-field write to an empty `list()` starts a record-like shape.
                CoreType::Tuple(items) if items.is_empty() => {
                    return Ok(CoreType::Record(vec![RecordField::new(
                        field_name, value_type,
                    )]));
                }
                _ => return Ok(prior_type.clone()),
            }
        }

        // A computed-key write (`x[[key]] <- v` with a non-literal key) cannot name a field, so it
        // refines the container's element type. A record-like or fixed-shape tuple keeps its
        // statically-known shape (widening it would discard precision the code has not given up);
        // the reachable list shapes join the written type into their element. Only a single-index
        // `[[<-` on the base counts — an `@slot` write, a `[<-`, or a multi-index `[[` does not.
        let is_computed_subset2 = matches!(
            &arena.get(lhs).kind,
            ExpressionKind::Subset2 { value, arguments }
                if *value == base_id
                    && matches!(
                        arguments.as_slice(),
                        [argument] if argument.name.is_none()
                    )
        );
        if !is_computed_subset2 {
            return Ok(prior_type.clone());
        }
        match prior_type {
            CoreType::Tuple(items) if items.is_empty() => {
                Ok(CoreType::NamedList(Box::new(value_type)))
            }
            CoreType::NamedList(element) => {
                Ok(CoreType::NamedList(Box::new(CoreType::union_of(vec![
                    (**element).clone(),
                    value_type,
                ]))))
            }
            CoreType::List(element) => Ok(CoreType::List(Box::new(CoreType::union_of(vec![
                (**element).clone(),
                value_type,
            ])))),
            _ => Ok(prior_type.clone()),
        }
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
            type_scheme: std::sync::Arc::new(TypeScheme::monomorphic(written)),
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
        let annotation = expression.annotation.as_deref();
        let value_expression = arena.get(value);

        // A parse hole proves nothing and its syntax error already marks the spot, so a checked
        // annotation on a broken value binds its DECLARED type unchecked — the definition keeps
        // its contract for dependents while the value is mid-edit — instead of demanding proof
        // from `Unknown` (a guaranteed false mismatch).
        if matches!(value_expression.kind, ExpressionKind::Missing)
            && let Some(annotation) = annotation
            && annotation.applies_to_binding()
            && let Annotation::Type { surface_type, .. } = annotation.annotation()
        {
            return match self.lower_annotation_surface_type(
                surface_type,
                type_definitions,
                Some(expression),
            ) {
                Ok(declared) => Ok(declared),
                Err(InferenceError::UnresolvedAnnotationType { .. }) => Ok(CoreType::Unknown),
                Err(error) => Err(error),
            };
        }

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
}

// Every symbol's exported binding — its last resolved simple assignment in document order,
// recursing into bare top-level `{ }` blocks — collected in one walk (the batch form of
// `naming::find_exported_binding`, which `exported_value_schemes` would otherwise run once per
// exported symbol).
fn collect_exported_bindings(
    module: &Module,
    local_naming: &NamesLocal,
    expressions: &[ExpressionId],
    exported: &mut BTreeMap<Symbol, BindingId>,
) {
    for expression_id in expressions {
        let kind = &module.arena.get(*expression_id).kind;
        if let Some(target) = kind.simple_assignment_target() {
            if let Some(binding_id) = local_naming
                .expression_resolutions
                .get(expression_id)
                .copied()
            {
                exported.insert(target, binding_id);
            }
        } else if let ExpressionKind::Block { expressions, .. } = kind {
            collect_exported_bindings(module, local_naming, expressions, exported);
        }
    }
}

// Letrec candidates: every top-level function-valued assignment, last writer per symbol (like the
// package symbol index). Only candidates on a reference CYCLE — a mutually-recursive group —
// become letrec members: a placeholder makes a member monomorphic within its own file (its later
// in-file uses constrain the shared variable instead of instantiating a scheme), a cost only
// recursion justifies.
fn module_letrec_candidates(
    module: &Module,
) -> BTreeMap<Symbol, (ExpressionId, ExpressionId, Range)> {
    let mut candidates: BTreeMap<Symbol, (ExpressionId, ExpressionId, Range)> = BTreeMap::new();
    for expression_id in &module.expressions {
        let expression = module.arena.get(*expression_id);
        let ExpressionKind::Assign {
            target: AssignTarget::Variable { symbol, .. },
            scope: AssignmentScope::Local,
            value,
        } = &expression.kind
        else {
            continue;
        };
        if !matches!(
            module.arena.get(*value).kind,
            ExpressionKind::Function { .. }
        ) {
            continue;
        }
        candidates.insert(*symbol, (*expression_id, *value, expression.range));
    }
    candidates
}

/// The module's letrec member symbols: top-level function-valued assignment targets on a *mutual*
/// reference cycle (two or more nodes; pure self-recursion deliberately keeps the tolerant
/// interface path). The whole-file walk pre-binds these to shared placeholder variables, so any
/// per-definition decomposition of the file is sound only when this set is empty.
pub fn module_letrec_member_symbols(module: &Module) -> BTreeSet<Symbol> {
    let candidates = module_letrec_candidates(module);
    module_letrec_member_symbols_from(module, &candidates)
}

// The reference edges among candidates, overapproximated by source-range containment: a `Symbol`
// expression lying inside a candidate's function value that names another candidate is an edge.
// (A local variable shadowing a candidate name inside a body adds a false edge — the
// overapproximation only widens the member set, never drops recursion.) One arena pass with a
// binary search per candidate-naming read: the candidate values are distinct top-level statements,
// so their ranges are disjoint and the last interval starting at or before a read is the only one
// that can contain it.
fn module_letrec_member_symbols_from(
    module: &Module,
    candidates: &BTreeMap<Symbol, (ExpressionId, ExpressionId, Range)>,
) -> BTreeSet<Symbol> {
    let mut edges: BTreeMap<Symbol, BTreeSet<Symbol>> = BTreeMap::new();
    for symbol in candidates.keys() {
        edges.insert(*symbol, BTreeSet::new());
    }
    let mut value_intervals: Vec<(usize, usize, Symbol)> = candidates
        .iter()
        .map(|(symbol, (_, value, _))| {
            let range = module.arena.get(*value).range;
            (range.start_byte, range.end_byte, *symbol)
        })
        .collect();
    value_intervals.sort_unstable();
    debug_assert!(
        value_intervals
            .windows(2)
            .all(|pair| pair[0].1 <= pair[1].0),
        "top-level candidate value ranges overlap"
    );
    for expression in module.arena.expressions() {
        let ExpressionKind::Symbol(read) = &expression.kind else {
            continue;
        };
        if !candidates.contains_key(read) {
            continue;
        }
        let position =
            value_intervals.partition_point(|(start, _, _)| *start <= expression.range.start_byte);
        let Some((start, end, owner)) = position.checked_sub(1).map(|i| value_intervals[i]) else {
            continue;
        };
        if expression.range.start_byte >= start
            && expression.range.end_byte <= end
            && let Some(referenced) = edges.get_mut(&owner)
        {
            referenced.insert(*read);
        }
    }
    // Only MUTUAL cycles (two or more members) get placeholders. A pure self-recursive function
    // keeps the tolerant fixed-point path: heterogeneous self-recursion — the idiomatic tree fold
    // `T = double | list[T]` — is untypeable in HM, and pinning its parameter through the
    // recursive call manufactures false positives at call sites; its `Unknown` is deliberate
    // gradual tolerance. A mutual group, by contrast, was previously pinned to `Unknown` wholesale
    // and false-errored annotated consumers.
    mutual_cycle_members(&edges)
}

/// The per-definition decomposition of a file for the interface fixed point, when it is provably
/// sound: every exported symbol mapped to its single top-level defining statement. `None` when the
/// whole-file walk could observe cross-statement state a per-definition check cannot reproduce.
///
/// A file decomposes exactly when, for every top-level name a definition might read, the walk's
/// value at the read equals the name's interface value. The conditions:
/// - no captured-write discovery re-pass (`top_level_capture_repass` forces two whole-file passes);
/// - no letrec members (mutual groups share placeholder variables across the whole file);
/// - every top-level simple assignment binds a distinct symbol exactly once, and its value is a
///   function literal or a scalar literal — both yield schemes fixed at the defining site (no free
///   inference variables a later statement could constrain), so the walk value, the exported
///   value, and the interface value coincide;
/// - no other top-level statement writes the top-level frame (no `Assign` outside nested function
///   or `local()` bodies): such a write could bind or reshape a slot a later definition reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SccDefinitionPlan {
    pub definitions: BTreeMap<Symbol, ExpressionId>,
}

pub fn scc_definition_plan(
    module: &Module,
    local_naming: &NamesLocal,
) -> Option<SccDefinitionPlan> {
    if local_naming.top_level_capture_repass {
        return None;
    }
    let mut definitions: BTreeMap<Symbol, ExpressionId> = BTreeMap::new();
    let top_level: BTreeSet<ExpressionId> = module.expressions.iter().copied().collect();
    for expression_id in &module.expressions {
        let expression = module.arena.get(*expression_id);
        match &expression.kind {
            ExpressionKind::Assign {
                target: AssignTarget::Variable { symbol, .. },
                scope: AssignmentScope::Local,
                value,
            } => {
                let value_kind = &module.arena.get(*value).kind;
                let fixed_at_site = matches!(
                    value_kind,
                    ExpressionKind::Function { .. }
                        | ExpressionKind::Double(_)
                        | ExpressionKind::Integer(_)
                        | ExpressionKind::Logical(_)
                        | ExpressionKind::Character(_)
                        | ExpressionKind::Null
                        | ExpressionKind::AtomicConstant(_)
                );
                if !fixed_at_site {
                    return None;
                }
                if !local_naming
                    .expression_resolutions
                    .contains_key(expression_id)
                {
                    return None;
                }
                if definitions.insert(*symbol, *expression_id).is_some() {
                    return None;
                }
            }
            ExpressionKind::Assign { .. } => return None,
            _ => {}
        }
    }
    // No top-frame write may hide outside the validated simple assignments: scan every `Assign` in
    // the arena and require it to be either one of the top-level statements above or nested inside
    // a function or `local()` body (its own frame). The frames' ranges nest properly, so the
    // outermost ones are disjoint and a sorted list answers containment with one binary search.
    let mut frame_intervals: Vec<(usize, usize)> = Vec::new();
    for expression in module.arena.expressions() {
        if matches!(
            expression.kind,
            ExpressionKind::Function { .. } | ExpressionKind::Local { .. }
        ) {
            frame_intervals.push((expression.range.start_byte, expression.range.end_byte));
        }
    }
    frame_intervals.sort_unstable();
    let mut outermost: Vec<(usize, usize)> = Vec::new();
    for (start, end) in frame_intervals {
        match outermost.last() {
            Some((_, last_end)) if end <= *last_end => {}
            _ => outermost.push((start, end)),
        }
    }
    for expression in module.arena.expressions() {
        if !matches!(expression.kind, ExpressionKind::Assign { .. }) {
            continue;
        }
        if top_level.contains(&expression.id) {
            continue;
        }
        let position =
            outermost.partition_point(|(start, _)| *start <= expression.range.start_byte);
        let covered = position
            .checked_sub(1)
            .is_some_and(|index| outermost[index].1 >= expression.range.end_byte);
        if !covered {
            return None;
        }
    }
    if !module_letrec_member_symbols(module).is_empty() {
        return None;
    }
    Some(SccDefinitionPlan { definitions })
}

/// The package globals a single top-level statement references: the file's non-local reads whose
/// expression lies within the statement's range (including reads inside nested function bodies).
/// This is the per-definition analog of the file-level referenced set `infer_file` imports.
pub fn statement_reference_symbols(
    module: &Module,
    local_naming: &NamesLocal,
    statement: ExpressionId,
) -> BTreeSet<Symbol> {
    let range = module.arena.get(statement).range;
    local_naming
        .non_locals
        .iter()
        .filter(|(expression_id, _)| {
            let read = module.arena.get(**expression_id).range;
            read.start_byte >= range.start_byte && read.end_byte <= range.end_byte
        })
        .map(|(_, symbol)| *symbol)
        .collect()
}

// The letrec candidates that lie on a mutual reference cycle: members of any strongly-connected
// component of two or more nodes in the candidate reference graph (iterative Tarjan). A self-edge
// alone never forms one, so pure self-recursion stays off the letrec path by construction.
fn mutual_cycle_members(edges: &BTreeMap<Symbol, BTreeSet<Symbol>>) -> BTreeSet<Symbol> {
    let symbols: Vec<Symbol> = edges.keys().copied().collect();
    let index_of: BTreeMap<Symbol, usize> = symbols
        .iter()
        .enumerate()
        .map(|(position, symbol)| (*symbol, position))
        .collect();
    let successors: Vec<Vec<usize>> = symbols
        .iter()
        .map(|symbol| {
            edges[symbol]
                .iter()
                .filter_map(|target| index_of.get(target).copied())
                .collect()
        })
        .collect();

    const UNVISITED: usize = usize::MAX;
    let node_count = symbols.len();
    let mut index = vec![UNVISITED; node_count];
    let mut low = vec![0usize; node_count];
    let mut on_stack = vec![false; node_count];
    let mut component_stack: Vec<usize> = Vec::new();
    let mut next_index = 0usize;
    struct Frame {
        node: usize,
        next_edge: usize,
    }
    let mut frames: Vec<Frame> = Vec::new();
    let mut members = BTreeSet::new();

    for start in 0..node_count {
        if index[start] != UNVISITED {
            continue;
        }
        index[start] = next_index;
        low[start] = next_index;
        next_index += 1;
        component_stack.push(start);
        on_stack[start] = true;
        frames.push(Frame {
            node: start,
            next_edge: 0,
        });
        while let Some(frame) = frames.last_mut() {
            let node = frame.node;
            if frame.next_edge < successors[node].len() {
                let target = successors[node][frame.next_edge];
                frame.next_edge += 1;
                if index[target] == UNVISITED {
                    index[target] = next_index;
                    low[target] = next_index;
                    next_index += 1;
                    component_stack.push(target);
                    on_stack[target] = true;
                    frames.push(Frame {
                        node: target,
                        next_edge: 0,
                    });
                } else if on_stack[target] {
                    low[node] = low[node].min(index[target]);
                }
            } else {
                if low[node] == index[node] {
                    let mut component = Vec::new();
                    loop {
                        let member = component_stack
                            .pop()
                            .expect("component stack is non-empty at a root");
                        on_stack[member] = false;
                        component.push(member);
                        if member == node {
                            break;
                        }
                    }
                    if component.len() >= 2 {
                        members.extend(component.into_iter().map(|member| symbols[member]));
                    }
                }
                frames.pop();
                if let Some(parent) = frames.last() {
                    let parent_node = parent.node;
                    low[parent_node] = low[parent_node].min(low[node]);
                }
            }
        }
    }
    members
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
        // The entry count doubles as the id counter (dense table), so one assertion covers both
        // leaked entries and leaked ids.
        let entry_count_before = state.entries.len();

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
        assert_eq!(
            state.entries.len(),
            entry_count_before,
            "no leaked entries or variable ids"
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
