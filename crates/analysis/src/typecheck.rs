use {
    crate::{
        document::DocumentId,
        hir::{
            Argument, DefinitionKind, Expression, ExpressionId, ExpressionKind, HirArena, Module,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EnvironmentKey {
    Local(BindingId),
    Global(Symbol),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleCheck {
    pub expression_types: Vec<CoreType>,
    pub expression_types_by_id: BTreeMap<ExpressionId, CoreType>,
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
    surface_type: SurfaceType,
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
                        surface_type: definition.definition.surface_type.clone(),
                    },
                );
            }
        }

        Self { definitions }
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
    accumulate_parameter_variances(
        &definition.surface_type,
        Variance::Covariant,
        &definition.type_parameters,
        &mut variances,
    );
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
            accumulate_parameter_variances(
                &function_type.return_type,
                polarity,
                parameters,
                variances,
            );
        }
        SurfaceType::Nullable(inner)
        | SurfaceType::Vector(inner)
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
];

impl InferenceState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fresh_variable(&mut self) -> InferenceVariableId {
        self.fresh_constrained_variable(Constraint::Unconstrained)
    }

    fn fresh_rigid_variable(&mut self, name: Symbol) -> InferenceVariableId {
        let variable = self.fresh_variable();
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
            CoreType::Nullable(inner) => nullable_type(self.substitute_rigid_names(inner)),
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
            CoreType::Function(function_type) => CoreType::Function(FunctionType::new(
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
    fn record_strict_origin(
        &mut self,
        expression_id: ExpressionId,
        range: Range,
        kind: StrictOriginKind,
    ) {
        if self.record_expression_types {
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
    //   likewise transient and deliberately excluded. M2.2 must keep its probes within this contract.
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
                        constraint: (*existing).max(constraint),
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
        self.environment.insert(
            EnvironmentKey::Global(symbol),
            Binding { type_scheme, range },
        );
    }

    pub fn bind_scheme(&mut self, symbol: Symbol, type_scheme: TypeScheme, range: Range) {
        self.bind_global_scheme(symbol, type_scheme, range);
    }

    fn bind_local_name(&mut self, binding_id: BindingId, core_type: CoreType, range: Range) {
        self.bind_local_scheme(binding_id, TypeScheme::monomorphic(core_type), range);
    }

    fn bind_local_scheme(&mut self, binding_id: BindingId, type_scheme: TypeScheme, range: Range) {
        self.environment.insert(
            EnvironmentKey::Local(binding_id),
            Binding { type_scheme, range },
        );
    }

    pub fn bind_builtin(&mut self, symbol: Symbol, builtin_kind: BuiltinKind) {
        self.builtins.insert(symbol, builtin_kind);
    }

    fn lookup_local_name(&self, binding_id: BindingId) -> Option<&Binding> {
        self.environment.get(&EnvironmentKey::Local(binding_id))
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
        let mut expression_types = Vec::with_capacity(module.expressions.len());
        let mut errors = Vec::new();

        for expression_id in &module.expressions {
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
                    if let ExpressionKind::Assign { target, .. } = &expression.kind {
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
                        }
                        if package_naming.global_bindings.contains_key(target) {
                            self.bind_global_scheme(*target, recovery_scheme, expression.range);
                        }
                    }
                }
            }
        }

        let expression_types_by_id = self.take_recorded_expression_types();
        // Keep only origins whose expression still resolves to `Unknown` in the final substitution.
        // This drops any origin whose `Unknown` was later overridden (for example a `@trust Any`
        // annotation makes the expression `Any`, which strict mode tolerates) and any origin under a
        // top-level statement that failed to type-check (its error is recorded separately and the
        // failed expression is never recorded here, so no double-report).
        let strict_origins = std::mem::take(&mut self.strict_origins)
            .into_iter()
            .filter(|origin| {
                expression_types_by_id.get(&origin.expression_id) == Some(&CoreType::Unknown)
            })
            .collect();
        ModuleCheck {
            expression_types,
            expression_types_by_id,
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
            if let ExpressionKind::Assign { target, .. } = &module.arena.get(*expression_id).kind
                && !symbols_in_order.contains(target)
            {
                symbols_in_order.push(*target);
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
        self.generalize(core_type)
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
            ExpressionKind::Assign { target, value } => {
                // Variables created while inferring the assigned value live one level above
                // the binding boundary, so generalization quantifies exactly the variables
                // that do not escape into the enclosing scope, with no environment walk.
                self.enter_level();
                let binding_type_result = self.infer_assign_binding_type(
                    *value,
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
                            *target,
                        ))
                        .is_some_and(|(binding_id, export_binding_id)| {
                            *binding_id == export_binding_id
                        })
                        && resolution_context
                            .package_naming
                            .global_bindings
                            .get(target)
                            == Some(&resolution_context.document_id);

                    if !is_current_document_winner {
                        if let Some(binding_id) = resolution_context
                            .local_naming
                            .expression_resolutions
                            .get(&expression.id)
                            .copied()
                        {
                            let generalized_scheme = self.generalize(binding_type.clone())?;
                            self.bind_local_scheme(
                                binding_id,
                                generalized_scheme,
                                expression.range,
                            );
                            return Ok(binding_type);
                        }

                        return Ok(binding_type);
                    }

                    self.environment.remove(&EnvironmentKey::Global(*target));
                    let generalized_scheme = self.generalize(binding_type.clone())?;
                    self.bind_global_scheme(*target, generalized_scheme, expression.range);
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
                } else {
                    self.bind_global_scheme(*target, generalized_scheme, expression.range);
                }
                Ok(binding_type)
            }
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
                expression.range,
                arena,
                resolution_context,
                type_definitions,
            ),
            ExpressionKind::While { condition, body } => self.infer_while_expression(
                arena.get(*condition),
                arena.get(*body),
                arena,
                resolution_context,
                type_definitions,
            ),
            ExpressionKind::Repeat { body } => self.infer_repeat_expression(
                arena.get(*body),
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
        let parent_environment = self.environment.clone();

        let expected_parameter_types = expected_function_type
            .as_ref()
            .map(flatten_expected_parameter_types)
            .filter(|types| types.len() == parameters.len());
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

        let inferred_return_type = self.infer_expression_with_context(
            arena.get(body),
            arena,
            resolution_context,
            type_definitions,
        )?;

        // The body's return value only needs to be *compatible* with the annotated return (covariant,
        // like an argument against a parameter), so a body returning `integer` satisfies a declared
        // `integer | NULL` or `integer[]`. A `<T>` return is a rigid skolem, so a body returning a
        // concrete type fails here. This is checked separately to report a focused return message.
        if let Some(expected_function_type) = &expected_function_type {
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
        let inferred_function_type =
            FunctionType::new(Vec::new(), named_parameter_types, inferred_return_type);

        self.environment = parent_environment;

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

        let inferred_consequence = self.infer_expression_with_context(
            consequence,
            arena,
            resolution_context,
            type_definitions,
        )?;
        let consequence_type = self.resolve(inferred_consequence)?;
        let Some(alternative) = alternative else {
            return Ok(nullable_type(consequence_type));
        };

        let inferred_alternative = self.infer_expression_with_context(
            alternative,
            arena,
            resolution_context,
            type_definitions,
        )?;
        let alternative_type = self.resolve(inferred_alternative)?;
        if consequence_type == alternative_type {
            return Ok(consequence_type);
        }

        match (&consequence_type, &alternative_type) {
            // Exactly one branch is `NULL`: the other becomes nullable.
            (CoreType::Null, _) => return Ok(nullable_type(alternative_type)),
            (_, CoreType::Null) => return Ok(nullable_type(consequence_type)),
            // An unmodelled branch makes the result unmodelled rather than a spurious mismatch,
            // matching how `Unknown` propagates through the rest of the checker.
            (CoreType::Unknown, _) | (_, CoreType::Unknown) => return Ok(CoreType::Unknown),
            _ => {}
        }

        // Both branches must share a type. Unifying them accepts the chooser idiom
        // `if (cond) a else b` where each branch is an inference variable, while still reporting a
        // clean mismatch (with resolved, user-facing types) for genuinely different concrete types.
        match self.unify(consequence_type.clone(), alternative_type.clone()) {
            Ok(unified_type) => self.resolve(unified_type),
            Err(_) => Err(InferenceError::TypeMismatch {
                expected: Box::new(self.resolve(consequence_type)?),
                actual: Box::new(self.resolve(alternative_type)?),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            }),
        }
    }

    fn infer_for_expression(
        &mut self,
        _expression_id: ExpressionId,
        variable: Symbol,
        sequence: &Expression,
        body: &Expression,
        range: Range,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
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
                expected: Box::new(CoreType::Vector(Atomic::Integer)),
                actual: Box::new(sequence_type),
                range: Some(range),
                expression_id: Some(sequence.id),
            });
        };

        if let Some(binding_id) = resolution_context.and_then(|context| {
            find_binding(context.local_naming, context.document_id, variable, range)
        }) {
            self.bind_local_name(binding_id, item_type, range);
            self.infer_expression_with_context(body, arena, resolution_context, type_definitions)?;
            self.environment.remove(&EnvironmentKey::Local(binding_id));
            return Ok(CoreType::Null);
        }

        let previous_binding = self
            .environment
            .get(&EnvironmentKey::Global(variable))
            .cloned();
        self.bind_global_name(variable, item_type, range);
        self.infer_expression_with_context(body, arena, resolution_context, type_definitions)?;

        if let Some(previous_binding) = previous_binding {
            self.environment
                .insert(EnvironmentKey::Global(variable), previous_binding);
        } else {
            self.environment.remove(&EnvironmentKey::Global(variable));
        }

        Ok(CoreType::Null)
    }

    fn infer_while_expression(
        &mut self,
        condition: &Expression,
        body: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        self.expect_scalar_logical(condition, arena, resolution_context, type_definitions)?;
        self.infer_expression_with_context(body, arena, resolution_context, type_definitions)?;
        Ok(CoreType::Null)
    }

    fn infer_repeat_expression(
        &mut self,
        body: &Expression,
        arena: &HirArena,
        resolution_context: Option<&ResolutionContext<'_>>,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        self.infer_expression_with_context(body, arena, resolution_context, type_definitions)?;
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
            (CoreType::Unknown, CoreType::Any) => Ok(true),
            (CoreType::Null, CoreType::Nullable(_)) => Ok(true),
            (other_type, CoreType::Nullable(inner_type)) => {
                self.check_compatibility(other_type, *inner_type, type_definitions, expression)
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
            (CoreType::Scalar(actual_atomic), CoreType::Vector(expected_atomic)) => {
                Ok(actual_atomic == expected_atomic)
            }
            (CoreType::NamedVector(actual_atomic), CoreType::Vector(expected_atomic)) => {
                Ok(actual_atomic == expected_atomic)
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

                let mut actual_parameters = actual_function
                    .parameters
                    .into_iter()
                    .map(|parameter| (parameter, false))
                    .collect::<Vec<_>>();
                actual_parameters.extend(
                    actual_function
                        .named_parameters
                        .into_iter()
                        .map(|parameter| (parameter.value, parameter.optional)),
                );

                let mut expected_parameters = expected_function
                    .parameters
                    .into_iter()
                    .map(|parameter| (parameter, false))
                    .collect::<Vec<_>>();
                expected_parameters.extend(
                    expected_function
                        .named_parameters
                        .into_iter()
                        .map(|parameter| (parameter.value, parameter.optional)),
                );

                for ((actual_param, actual_optional), (expected_param, expected_optional)) in
                    actual_parameters.into_iter().zip(expected_parameters)
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
            SurfaceType::Nullable(inner_type) => {
                Ok(nullable_type(self.lower_surface_type_with_substitutions(
                    inner_type,
                    substitutions,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )?))
            }
            SurfaceType::Scalar(atomic) => Ok(CoreType::Scalar(*atomic)),
            SurfaceType::Named(name, arguments) => {
                if arguments.is_empty() {
                    if let Some(core_type) = substitutions.get(name) {
                        return Ok(core_type.clone());
                    }
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

                match type_definition.kind {
                    DefinitionKind::Type => Ok(CoreType::Nominal(*name, lowered_arguments)),
                    DefinitionKind::Alias => {
                        if !expanding_aliases.insert(*name) {
                            return Err(alias_cycle_error(*name, expression));
                        }

                        let lowered_alias =
                            if type_definition.type_parameters.len() != lowered_arguments.len() {
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

                                self.lower_surface_type_with_substitutions(
                                    &type_definition.surface_type,
                                    &nested_substitutions,
                                    expanding_aliases,
                                    type_definitions,
                                    expression,
                                )
                            };

                        expanding_aliases.remove(name);
                        lowered_alias
                    }
                }
            }
            SurfaceType::Vector(inner_type) => {
                match self.lower_surface_type_with_substitutions(
                    inner_type,
                    substitutions,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )? {
                    CoreType::Scalar(atomic) => Ok(CoreType::Vector(atomic)),
                    other_type => Ok(CoreType::List(Box::new(other_type))),
                }
            }
            SurfaceType::NamedVector(inner_type) => {
                match self.lower_surface_type_with_substitutions(
                    inner_type,
                    substitutions,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )? {
                    CoreType::Scalar(atomic) => Ok(CoreType::NamedVector(atomic)),
                    other_type => Ok(CoreType::NamedList(Box::new(other_type))),
                }
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
                        Ok(RecordField::new(
                            field.name,
                            self.lower_surface_type_with_substitutions(
                                &field.value,
                                substitutions,
                                expanding_aliases,
                                type_definitions,
                                expression,
                            )?,
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
            SurfaceType::Function(function_type) => Ok(CoreType::Function(FunctionType::new(
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
                self.lower_surface_type_with_substitutions(
                    &function_type.return_type,
                    substitutions,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )?,
            ))),
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
                    nested_type_parameters.insert(
                        *type_parameter,
                        CoreType::Variable(self.fresh_rigid_variable(*type_parameter)),
                    );
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

        match self.lower_surface_type_with_substitutions(
            &type_definition.surface_type,
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
            CoreType::Nullable(inner_type) => {
                let resolved_inner_type = self.resolve(*inner_type)?;
                Ok(nullable_type(resolved_inner_type))
            }
            CoreType::Nominal(symbol, type_arguments) => {
                let mut resolved_type_arguments = Vec::with_capacity(type_arguments.len());
                for type_argument in type_arguments {
                    resolved_type_arguments.push(self.resolve(type_argument)?);
                }
                Ok(CoreType::Nominal(symbol, resolved_type_arguments))
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
                    resolved_fields.push(RecordField::new(field.name, self.resolve(field.value)?));
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
            (CoreType::Nullable(left_type), CoreType::Nullable(right_type)) => {
                let unified_type = self.unify_internal(*left_type, *right_type, expression)?;
                Ok(nullable_type(unified_type))
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
            (CoreType::Nullable(inner_type), CoreType::Null)
            | (CoreType::Null, CoreType::Nullable(inner_type)) => {
                Ok(CoreType::Nullable(inner_type))
            }
            (CoreType::Scalar(left_atomic), CoreType::Scalar(right_atomic))
                if left_atomic == right_atomic =>
            {
                Ok(CoreType::Scalar(left_atomic))
            }
            (CoreType::Vector(left_atomic), CoreType::Vector(right_atomic))
                if left_atomic == right_atomic =>
            {
                Ok(CoreType::Vector(left_atomic))
            }
            (CoreType::NamedVector(left_atomic), CoreType::NamedVector(right_atomic))
                if left_atomic == right_atomic =>
            {
                Ok(CoreType::NamedVector(left_atomic))
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
            CoreType::Nullable(inner_type) => self.occurs_in(variable, &inner_type),
            CoreType::Nominal(_, type_arguments) => {
                for type_argument in type_arguments {
                    if self.occurs_in(variable, &type_argument)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
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
                actual: resolved_left,
                range: arg0.range,
                expression_id: arg0.id,
            });
        }
        if let NumericOperand::Invalid = right {
            return Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Numeric,
                actual: resolved_right,
                range: arg1.range,
                expression_id: arg1.id,
            });
        }
        if matches!(left, NumericOperand::AnyUnknown) || matches!(right, NumericOperand::AnyUnknown)
        {
            return Ok(CoreType::Unknown);
        }

        let result_shape = if left.is_vector() || right.is_vector() {
            OperandShape::Vector
        } else {
            OperandShape::Scalar
        };

        // Constrain every flexible operand to be numeric, collapsing them onto one representative
        // variable so `x + y` ties the two operands together.
        let mut flexible_variable: Option<InferenceVariableId> = None;
        for operand in [left, right] {
            if let NumericOperand::Variable(variable) = operand {
                flexible_variable = Some(match flexible_variable {
                    Some(existing) => match self
                        .unify(CoreType::Variable(existing), CoreType::Variable(variable))?
                    {
                        CoreType::Variable(unified) => unified,
                        _ => existing,
                    },
                    None => variable,
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

        if let NumericResultAtomic::AlwaysDouble = numeric_result_atomic {
            return Ok(core_type_for_shape(result_shape, Atomic::Double));
        }

        // Promote: a concrete `double` anywhere forces `double`.
        if left.concrete_atomic() == Some(Atomic::Double)
            || right.concrete_atomic() == Some(Atomic::Double)
        {
            return Ok(core_type_for_shape(result_shape, Atomic::Double));
        }

        match (result_shape, flexible_variable) {
            // `x + 1L` (and `x + y`) stay polymorphic over the numeric operand: integer promotes
            // to whatever the variable resolves to, so the scalar result is the variable itself.
            (OperandShape::Scalar, Some(variable)) => Ok(CoreType::Variable(variable)),
            // A vector result cannot carry an unresolved atomic, so a flexible operand defaults to
            // `double` here.
            (OperandShape::Vector, Some(variable)) => {
                self.bind_variable(variable, CoreType::Scalar(Atomic::Double), Some(expression))?;
                Ok(CoreType::Vector(Atomic::Double))
            }
            // Both operands were concrete integers.
            (shape, None) => Ok(core_type_for_shape(shape, Atomic::Integer)),
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
            NumericOperand::Variable(variable) => {
                self.constrain_type(
                    CoreType::Variable(variable),
                    Constraint::Numeric,
                    Some(value),
                )?;
                Ok(CoreType::Variable(variable))
            }
            NumericOperand::AnyUnknown => Ok(CoreType::Unknown),
            NumericOperand::Invalid => Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Numeric,
                actual: resolved_type,
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
            CoreType::Vector(Atomic::Logical) | CoreType::NamedVector(Atomic::Logical) => {
                Ok(CoreType::Vector(Atomic::Logical))
            }
            CoreType::Any | CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Variable(_) => {
                self.unify_with_context(CoreType::Scalar(Atomic::Logical), resolved_type, value)?;
                Ok(CoreType::Scalar(Atomic::Logical))
            }
            other_type => Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Logical,
                actual: other_type,
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

        let left_parts = comparison_operand_parts(&resolved_left);
        let right_parts = comparison_operand_parts(&resolved_right);
        let left_is_variable = matches!(resolved_left, CoreType::Variable(_));
        let right_is_variable = matches!(resolved_right, CoreType::Variable(_));

        if left_parts.is_none() && !left_is_variable {
            return Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Comparable,
                actual: resolved_left,
                range: arg0.range,
                expression_id: arg0.id,
            });
        }
        if right_parts.is_none() && !right_is_variable {
            return Err(InferenceError::InvalidOperand {
                expected: OperandExpectation::Comparable,
                actual: resolved_right,
                range: arg1.range,
                expression_id: arg1.id,
            });
        }

        // Two concrete operands must belong to the same comparison family.
        if let (Some((_, left_family)), Some((_, right_family))) = (left_parts, right_parts)
            && left_family != right_family
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
        if left_is_variable && matches!(right_parts, Some((_, ComparisonFamily::Numeric))) {
            self.constrain_type(resolved_left.clone(), Constraint::Numeric, Some(arg0))?;
        }
        if right_is_variable && matches!(left_parts, Some((_, ComparisonFamily::Numeric))) {
            self.constrain_type(resolved_right.clone(), Constraint::Numeric, Some(arg1))?;
        }

        let result_shape = if matches!(left_parts, Some((OperandShape::Vector, _)))
            || matches!(right_parts, Some((OperandShape::Vector, _)))
        {
            OperandShape::Vector
        } else {
            OperandShape::Scalar
        };
        Ok(core_type_for_shape(result_shape, Atomic::Logical))
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
                        actual: other_type,
                        range: argument_expression.range,
                        expression_id: argument_expression.id,
                    });
                }
            }
        }

        Ok(CoreType::Vector(result_atomic))
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
                actual_type: other_type,
                range: callee.range,
                expression_id: callee.id,
            }),
        }
    }

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
        let return_type = *function_type.return_type;
        let mut next_positional_index = 0;
        let mut remaining_named_parameters = function_type.named_parameters;

        for argument in arguments {
            let arg_expr = arena.get(argument.expression);
            let inferred_argument = self.infer_expression_with_context(
                arg_expr,
                arena,
                resolution_context,
                type_definitions,
            )?;
            if let Some(name) = argument.name {
                let Some(parameter_index) = remaining_named_parameters
                    .iter()
                    .position(|parameter| parameter.name == name)
                else {
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

            if !remaining_named_parameters.is_empty() {
                let parameter = remaining_named_parameters.remove(0);
                self.check_argument(
                    parameter.value,
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
        if arguments.len() != 1 || arguments[0].name.is_some() {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 1,
                actual: arguments.len(),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        let inferred_value =
            self.infer_expression_with_context(value, arena, resolution_context, type_definitions)?;
        let value_type = self.resolve_structural(inferred_value, type_definitions, Some(value))?;
        let arg0_expr = arena.get(arguments[0].expression);
        self.infer_expression_with_context(arg0_expr, arena, resolution_context, type_definitions)?;

        let unsupported = |actual: CoreType| InferenceError::UnsupportedSubset {
            actual: Box::new(actual),
            range: expression.range,
            expression_id: expression.id,
        };
        match value_type {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::List(item_type) => Ok(CoreType::List(item_type)),
            CoreType::NamedList(item_type) => Ok(CoreType::NamedList(item_type)),
            CoreType::Tuple(items) => match homogeneous_structural_item_type(&items) {
                Some(item_type) => Ok(CoreType::List(Box::new(item_type))),
                None => Err(unsupported(CoreType::Tuple(items))),
            },
            CoreType::Record(fields) => {
                let field_types = fields
                    .iter()
                    .map(|field| field.value.clone())
                    .collect::<Vec<_>>();
                match homogeneous_structural_item_type(&field_types) {
                    Some(item_type) => Ok(CoreType::NamedList(Box::new(item_type))),
                    None => Err(unsupported(CoreType::Record(fields))),
                }
            }
            other_type => Err(unsupported(other_type)),
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
        if arguments.len() != 1 || arguments[0].name.is_some() {
            return Err(InferenceError::FunctionArityMismatch {
                expected: 1,
                actual: arguments.len(),
                range: Some(expression.range),
                expression_id: Some(expression.id),
            });
        }

        let inferred_value =
            self.infer_expression_with_context(value, arena, resolution_context, type_definitions)?;
        let value_type = self.resolve_structural(inferred_value, type_definitions, Some(value))?;
        let index_expression = arena.get(arguments[0].expression);
        self.infer_expression_with_context(
            index_expression,
            arena,
            resolution_context,
            type_definitions,
        )?;

        match value_type {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::Scalar(atomic) | CoreType::Vector(atomic) => Ok(CoreType::Scalar(atomic)),
            CoreType::NamedVector(atomic) => {
                if literal_name_symbol(index_expression).is_some() {
                    Ok(nullable_type(CoreType::Scalar(atomic)))
                } else {
                    Ok(CoreType::Scalar(atomic))
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

        match value_type {
            CoreType::Unknown => Ok(CoreType::Unknown),
            CoreType::Any => Ok(CoreType::Any),
            CoreType::NamedVector(atomic) => Ok(nullable_type(CoreType::Scalar(atomic))),
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

            let Some(current_atomic) = combine_operand_atomic(&resolved_argument) else {
                return Err(InferenceError::TypeMismatch {
                    expected: Box::new(CoreType::Scalar(Atomic::Integer)),
                    actual: Box::new(resolved_argument.clone()),
                    range: Some(arg_expr.range),
                    expression_id: Some(arg_expr.id),
                });
            };

            item_atomic = Some(match item_atomic {
                Some(previous_atomic) => promote_combine_atomic(previous_atomic, current_atomic)
                    .ok_or_else(|| InferenceError::TypeMismatch {
                        expected: Box::new(CoreType::Scalar(previous_atomic)),
                        actual: Box::new(resolved_argument.clone()),
                        range: Some(arg_expr.range),
                        expression_id: Some(arg_expr.id),
                    })?,
                None => current_atomic,
            });
        }

        if !saw_non_null_argument {
            return Ok(CoreType::Null);
        }
        let combined_atomic = item_atomic.unwrap_or(Atomic::Integer);
        if all_arguments_are_named {
            Ok(CoreType::NamedVector(combined_atomic))
        } else {
            Ok(CoreType::Vector(combined_atomic))
        }
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
                in_type: core_type,
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
                constraint: (*constraint).max(redirected_constraint),
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
            CoreType::Nullable(inner_type) => Ok(nullable_type(
                self.instantiate_core_type(inner_type, substitutions)?,
            )),
            CoreType::Scalar(atomic) => Ok(CoreType::Scalar(*atomic)),
            CoreType::Nominal(symbol, type_arguments) => {
                let mut instantiated_type_arguments = Vec::with_capacity(type_arguments.len());
                for type_argument in type_arguments {
                    instantiated_type_arguments
                        .push(self.instantiate_core_type(type_argument, substitutions)?);
                }
                Ok(CoreType::Nominal(*symbol, instantiated_type_arguments))
            }
            CoreType::Vector(atomic) => Ok(CoreType::Vector(*atomic)),
            CoreType::NamedVector(atomic) => Ok(CoreType::NamedVector(*atomic)),
            CoreType::List(item_type) => Ok(CoreType::List(Box::new(
                self.instantiate_core_type(item_type, substitutions)?,
            ))),
            CoreType::NamedList(item_type) => Ok(CoreType::NamedList(Box::new(
                self.instantiate_core_type(item_type, substitutions)?,
            ))),
            CoreType::Record(fields) => {
                let mut instantiated_fields = Vec::with_capacity(fields.len());
                for field in fields {
                    instantiated_fields.push(RecordField::new(
                        field.name,
                        self.instantiate_core_type(&field.value, substitutions)?,
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

                let instantiated_return_type =
                    self.instantiate_core_type(&function_type.return_type, substitutions)?;

                Ok(CoreType::Function(FunctionType::new(
                    instantiated_parameters,
                    instantiated_named_parameters,
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
                        constraint: Constraint::Numeric,
                    }) if *level > self.current_level
                ) {
                    self.bind_variable(variable, CoreType::Scalar(Atomic::Double), None)?;
                    return self.resolve(CoreType::Variable(variable));
                }
                Ok(CoreType::Variable(variable))
            }
            CoreType::Nullable(inner_type) => {
                Ok(nullable_type(self.default_free_numeric(*inner_type)?))
            }
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
            CoreType::Any
            | CoreType::Unknown
            | CoreType::Null
            | CoreType::Scalar(_)
            | CoreType::Vector(_)
            | CoreType::NamedVector(_) => Ok(BTreeSet::new()),
            CoreType::Nullable(inner_type) => self.free_type_variables_in_core_type(&inner_type),
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

        let resolved_return_type = self.resolve(*function_type.return_type)?;

        Ok(FunctionType::new(
            resolved_parameters,
            resolved_named_parameters,
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
        if left_total != right_total {
            return Err(InferenceError::FunctionArityMismatch {
                expected: left_total,
                actual: right_total,
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

        let unified_return_type = self.unify_internal(
            *left_function.return_type,
            *right_function.return_type,
            expression,
        )?;

        Ok(FunctionType::new(
            unified_parameters,
            unified_named_parameters,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandExpectation {
    Numeric,
    ScalarNumeric,
    Logical,
    Comparable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComparisonFamily {
    Numeric,
    Character,
    Logical,
}

fn comparison_operand_parts(core_type: &CoreType) -> Option<(OperandShape, ComparisonFamily)> {
    let (shape, atomic) = match core_type {
        CoreType::Scalar(atomic) => (OperandShape::Scalar, *atomic),
        CoreType::Vector(atomic) | CoreType::NamedVector(atomic) => (OperandShape::Vector, *atomic),
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

// How an operand of an arithmetic operator classifies: a concrete numeric shape, a still-flexible
// inference variable (which becomes numeric-constrained), an `Any`/`Unknown` short-circuit, or a
// hard error.
#[derive(Debug, Clone, Copy)]
enum NumericOperand {
    Concrete(OperandShape, Atomic),
    Variable(InferenceVariableId),
    AnyUnknown,
    Invalid,
}

impl NumericOperand {
    fn is_vector(self) -> bool {
        matches!(self, NumericOperand::Concrete(OperandShape::Vector, _))
    }

    fn concrete_atomic(self) -> Option<Atomic> {
        match self {
            NumericOperand::Concrete(_, atomic) => Some(atomic),
            _ => None,
        }
    }
}

fn classify_numeric_operand(core_type: &CoreType) -> NumericOperand {
    if let Some((shape, atomic)) = numeric_operand_parts(core_type) {
        return NumericOperand::Concrete(shape, atomic);
    }
    match core_type {
        CoreType::Variable(variable) => NumericOperand::Variable(*variable),
        CoreType::Any | CoreType::Unknown => NumericOperand::AnyUnknown,
        _ => NumericOperand::Invalid,
    }
}

fn constraint_is_satisfied(constraint: Constraint, core_type: &CoreType) -> bool {
    match constraint {
        Constraint::Unconstrained => true,
        Constraint::Numeric => is_numeric_core_type(core_type),
    }
}

fn is_numeric_core_type(core_type: &CoreType) -> bool {
    matches!(
        core_type,
        CoreType::Scalar(Atomic::Integer | Atomic::Double)
            | CoreType::Vector(Atomic::Integer | Atomic::Double)
            | CoreType::NamedVector(Atomic::Integer | Atomic::Double)
    )
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
        CoreType::Scalar(Atomic::Integer) => Some((OperandShape::Scalar, Atomic::Integer)),
        CoreType::Scalar(Atomic::Double) => Some((OperandShape::Scalar, Atomic::Double)),
        CoreType::Vector(Atomic::Integer) => Some((OperandShape::Vector, Atomic::Integer)),
        CoreType::Vector(Atomic::Double) => Some((OperandShape::Vector, Atomic::Double)),
        CoreType::NamedVector(Atomic::Integer) => Some((OperandShape::Vector, Atomic::Integer)),
        CoreType::NamedVector(Atomic::Double) => Some((OperandShape::Vector, Atomic::Double)),
        _ => None,
    }
}

fn combine_operand_atomic(core_type: &CoreType) -> Option<Atomic> {
    match core_type {
        CoreType::Scalar(atomic) | CoreType::Vector(atomic) | CoreType::NamedVector(atomic) => {
            Some(*atomic)
        }
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
        OperandShape::Vector => CoreType::Vector(atomic),
    }
}

fn nullable_type(core_type: CoreType) -> CoreType {
    match core_type {
        CoreType::Null => CoreType::Null,
        CoreType::Nullable(inner_type) => CoreType::Nullable(inner_type),
        other_type => CoreType::Nullable(Box::new(other_type)),
    }
}

fn iterable_item_type(core_type: &CoreType) -> Option<CoreType> {
    match core_type {
        CoreType::Scalar(atomic) | CoreType::Vector(atomic) | CoreType::NamedVector(atomic) => {
            Some(CoreType::Scalar(*atomic))
        }
        CoreType::List(item_type) | CoreType::NamedList(item_type) => Some((**item_type).clone()),
        CoreType::Tuple(items) => homogeneous_structural_item_type(items),
        CoreType::Record(fields) => homogeneous_structural_item_type(
            &fields
                .iter()
                .map(|field| field.value.clone())
                .collect::<Vec<_>>(),
        ),
        _ => None,
    }
}

fn homogeneous_structural_item_type(items: &[CoreType]) -> Option<CoreType> {
    let first_item = items.first()?.clone();
    if items.iter().skip(1).all(|item| *item == first_item) {
        Some(first_item)
    } else {
        None
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
        CoreType::Nullable(inner_type) => {
            nullable_type(import_core_type(inner_type, substitutions))
        }
        CoreType::Scalar(atomic) => CoreType::Scalar(*atomic),
        CoreType::Nominal(symbol, type_arguments) => CoreType::Nominal(
            *symbol,
            type_arguments
                .iter()
                .map(|type_argument| import_core_type(type_argument, substitutions))
                .collect(),
        ),
        CoreType::Vector(atomic) => CoreType::Vector(*atomic),
        CoreType::NamedVector(atomic) => CoreType::NamedVector(*atomic),
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
        CoreType::Function(function_type) => CoreType::Function(FunctionType::new(
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

fn flatten_expected_parameter_types(function_type: &FunctionType<CoreType>) -> Vec<CoreType> {
    let mut parameter_types =
        Vec::with_capacity(function_type.parameters.len() + function_type.named_parameters.len());
    parameter_types.extend(function_type.parameters.iter().cloned());
    parameter_types.extend(
        function_type
            .named_parameters
            .iter()
            .map(|parameter| parameter.value.clone()),
    );
    parameter_types
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
        actual_type: CoreType,
        range: Range,
        expression_id: ExpressionId,
    },
    OccursCheckFailed {
        variable: InferenceVariableId,
        in_type: CoreType,
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
    ConstraintViolation {
        constraint: Constraint,
        actual: Box<CoreType>,
        range: Option<Range>,
        expression_id: Option<ExpressionId>,
    },
    InvalidOperand {
        expected: OperandExpectation,
        actual: CoreType,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptKind {
    Position,
    FieldName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub type_scheme: TypeScheme,
    pub range: Range,
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
        let rigid = inference_state.fresh_rigid_variable(name);
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
