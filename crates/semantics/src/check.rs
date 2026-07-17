//! The expression inference walk: Hindley–Milner over one item's HIR.
//!
//! The environment is keyed by naming's variable slots and mirrors the mutable
//! slot model: every write funnels through an undo log so branches roll back
//! without cloning, and merge points join entries (equal keeps; different
//! joins as a monotype union). Function-valued assignments generalize at the
//! binding (level-gated so escaped variables stay monomorphic); reads
//! instantiate schemes with fresh variables. Arithmetic rides the numeric
//! constraint; comparisons and logic produce logicals; `if` joins branches by
//! unify-else-union exactly like the legacy contract.
//!
//! This is the foundation walk: parameter-position coercions, overload sets,
//! `#:` annotation enforcement, strict origins, and cross-item schemes layer
//! on next — each a separate slice over this structure.

use crate::Db;
use crate::hir::{
    Argument, BinaryOperator, ExprId, ExpressionKind, LiteralKind, Module, UnaryOperator,
};
use crate::infer::{Entry, InferenceTable, UnifyError};
use crate::naming::{BindingId, ItemNaming};
use crate::types::{
    Atomic, Constraint, FunctionType, Name, Ty, TyKind, TypeScheme, scalar, union_of,
};
use rustc_hash::FxHashMap;
use syntax::TextRange;

/// A structured type finding; rendering to wording happens at the diagnostic
/// edge.
#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)]
pub struct TypeError<'db> {
    pub range: TextRange,
    pub kind: TypeErrorKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)]
pub enum TypeErrorKind<'db> {
    Mismatch {
        expected: Ty<'db>,
        found: Ty<'db>,
    },
    NotAFunction {
        found: Ty<'db>,
    },
    /// The call's argument count cannot fill the function's formals (too many
    /// positionals, or a required formal left unfilled).
    ArityMismatch {
        expected: usize,
        found: usize,
    },
    UnknownArgument {
        name: String,
    },
    /// An annotation declares a parameter the definition has no formal for.
    AnnotationParameterMismatch {
        name: String,
    },
    /// A constraint (numeric, atomic) rejected the value.
    ConstraintViolation {
        constraint: Constraint,
        found: Ty<'db>,
    },
    /// No candidate of an overloaded stub name accepted the arguments; carries
    /// the first candidate's failure for a concrete lead.
    NoMatchingOverload {
        name: String,
        candidates: usize,
        first: Option<Box<TypeError<'db>>>,
    },
    InfiniteType,
}

/// The result of checking one item.
#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)]
pub struct ItemCheck<'db> {
    /// Per-expression resolved types (post-substitution).
    pub expression_types: FxHashMap<ExprId, Ty<'db>>,
    pub errors: Vec<TypeError<'db>>,
    /// The generalized scheme of the item's top-level binding value, when the
    /// item is a definition.
    pub scheme: Option<TypeScheme<'db>>,
}

/// Resolver for names that are not item-local: package globals and the stdlib
/// stub corpus.
pub trait GlobalEnv<'db> {
    fn scheme(&self, name: &str) -> Option<TypeScheme<'db>>;

    /// The full ordered overload-candidate set of a name, `None` when the name
    /// has at most one candidate or a package/local definition wins over the
    /// stub set.
    fn overloads(&self, name: &str) -> Option<Vec<TypeScheme<'db>>> {
        let _ = name;
        None
    }
}

pub fn check_item<'db>(db: &'db dyn Db, module: &Module, naming: &ItemNaming) -> ItemCheck<'db> {
    check_item_with_annotation(db, module, naming, None, None)
}

pub fn check_item_with_annotation<'db>(
    db: &'db dyn Db,
    module: &Module,
    naming: &ItemNaming,
    annotation: Option<&crate::annotations::Annotation<'db>>,
    globals: Option<&dyn GlobalEnv<'db>>,
) -> ItemCheck<'db> {
    let mut context = Checker {
        db,
        module,
        naming,
        globals,
        table: InferenceTable::default(),
        environment: Environment::default(),
        scheme_arena: Vec::new(),
        rigid_constraints: FxHashMap::default(),
        recorded: FxHashMap::default(),
        errors: Vec::new(),
        overload_probe_depth: 0,
    };
    let mut scheme = None;
    if let Some(root) = module.root {
        let declared_fn = annotation.filter(|a| !a.trusted).and_then(|a| {
            let declared = a.declared.as_ref()?;
            match declared.body.kind(db) {
                TyKind::Function(function) => Some((declared.clone(), function.clone())),
                _ => None,
            }
        });
        match (&module.expression(root).kind, declared_fn) {
            // A declared function annotation on a function definition: check
            // the body under the declared parameter types (rigid — they
            // refuse to bind), and check the result against the declared
            // return. The declared scheme wins as the export.
            (ExpressionKind::Assign { value, .. }, Some((declared, function)))
                if matches!(
                    module.expression(*value).kind,
                    ExpressionKind::Function { .. }
                ) =>
            {
                for (name, constraint) in &declared.binders {
                    context.rigid_constraints.insert(*name, *constraint);
                }
                context.check_declared_function(*value, &function);
                context.recorded.insert(root, declared.body);
                scheme = Some(declared);
            }
            (ExpressionKind::Assign { value, .. }, _) => {
                let root_ty = context.infer(root);
                let value_ty = context.recorded.get(value).copied().unwrap_or(root_ty);
                scheme = match annotation.and_then(|a| a.declared.clone()) {
                    // A declared non-function type (or a trusted one): the
                    // declaration is the contract.
                    Some(declared) => {
                        if annotation.is_some_and(|a| !a.trusted) {
                            let expected = declared.body;
                            if matches!(
                                declared.body.kind(db),
                                TyKind::Scalar(_)
                                    | TyKind::Null
                                    | TyKind::Vector(_)
                                    | TyKind::List(_)
                                    | TyKind::Union(_)
                            ) {
                                let range = module.expression(*value).range;
                                context.unify_or_report(range, expected, value_ty);
                            }
                        }
                        Some(declared)
                    }
                    None => Some(context.generalize(value_ty)),
                };
            }
            _ => {
                context.infer(root);
            }
        }
    }
    let expression_types = context
        .recorded
        .iter()
        .map(|(&id, &ty)| (id, context.table.resolve(context.db, ty)))
        .collect();
    ItemCheck {
        expression_types,
        errors: context.errors,
        scheme,
    }
}

/// The slot environment: an undo-logged map from binding slots to types.
#[derive(Debug, Default)]
struct Environment<'db> {
    entries: FxHashMap<BindingId, EnvEntry<'db>>,
    undo: Vec<(BindingId, Option<EnvEntry<'db>>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvEntry<'db> {
    /// A monotype slot value.
    Mono(Ty<'db>),
    /// A generalized (let-bound function) scheme, stored by index to keep the
    /// entry `Copy`; schemes live in the checker's arena.
    Scheme(u32),
}

impl<'db> Environment<'db> {
    fn get(&self, slot: BindingId) -> Option<EnvEntry<'db>> {
        self.entries.get(&slot).copied()
    }

    fn set(&mut self, slot: BindingId, entry: EnvEntry<'db>) {
        let previous = self.entries.insert(slot, entry);
        self.undo.push((slot, previous));
    }

    fn mark(&self) -> usize {
        self.undo.len()
    }

    fn rollback(&mut self, mark: usize) {
        while self.undo.len() > mark {
            let (slot, previous) = self.undo.pop().expect("undo length checked");
            match previous {
                Some(entry) => {
                    self.entries.insert(slot, entry);
                }
                None => {
                    self.entries.remove(&slot);
                }
            }
        }
    }

    /// The entries written since `mark`, deduplicated to their latest value.
    fn writes_since(&self, mark: usize) -> Vec<(BindingId, Option<EnvEntry<'db>>)> {
        let mut seen = std::collections::BTreeSet::new();
        let mut writes = Vec::new();
        for (slot, _) in self.undo[mark..].iter() {
            if seen.insert(*slot) {
                writes.push((*slot, self.entries.get(slot).copied()));
            }
        }
        writes
    }
}

struct Checker<'db, 'a> {
    db: &'db dyn Db,
    module: &'a Module,
    naming: &'a ItemNaming,
    globals: Option<&'a dyn GlobalEnv<'db>>,
    table: InferenceTable<'db>,
    environment: Environment<'db>,
    scheme_arena: Vec<TypeScheme<'db>>,
    /// Declared constraints of in-scope rigid binders (`<T: numeric>`).
    rigid_constraints: FxHashMap<Name<'db>, Constraint>,
    recorded: FxHashMap<ExprId, Ty<'db>>,
    errors: Vec<TypeError<'db>>,
    /// Non-zero while a strict overload-selection round probes a candidate:
    /// the literal-as-integer courtesy is off, so it cannot decide which
    /// candidate wins (exact matches outrank conversions).
    overload_probe_depth: u32,
}

/// One call argument, inferred exactly once before any signature matching, so
/// an overload probe can re-match without re-running expression inference.
struct CallArgument<'db> {
    name: Option<String>,
    /// `None` is a positional hole (`f(, x)`).
    ty: Option<Ty<'db>>,
    range: TextRange,
    /// The argument is a whole-number double literal (`1`, `2.0`) — eligible
    /// for the literal-as-integer courtesy.
    whole_double: bool,
}

impl<'db> Checker<'db, '_> {
    fn record(&mut self, id: ExprId, ty: Ty<'db>) -> Ty<'db> {
        self.recorded.insert(id, ty);
        ty
    }

    fn unknown(&self) -> Ty<'db> {
        crate::types::unknown(self.db)
    }

    fn fresh(&mut self, constraint: Constraint) -> Ty<'db> {
        self.table.fresh_ty(self.db, constraint)
    }

    fn unify_or_report(&mut self, range: TextRange, expected: Ty<'db>, found: Ty<'db>) {
        if let Err(error) = self.table.unify(self.db, expected, found) {
            self.report_unify(range, error);
        }
    }

    fn report_unify(&mut self, range: TextRange, error: UnifyError<'db>) {
        let kind = match error {
            UnifyError::Mismatch(expected, found) => TypeErrorKind::Mismatch {
                expected: self.table.resolve(self.db, expected),
                found: self.table.resolve(self.db, found),
            },
            UnifyError::Occurs(..) => TypeErrorKind::InfiniteType,
            UnifyError::ConstraintRejected(constraint, found) => {
                TypeErrorKind::ConstraintViolation {
                    constraint,
                    found: self.table.resolve(self.db, found),
                }
            }
        };
        self.errors.push(TypeError { range, kind });
    }

    fn infer(&mut self, id: ExprId) -> Ty<'db> {
        let expression = self.module.expression(id).clone();
        let range = expression.range;
        let ty = match &expression.kind {
            ExpressionKind::Missing => self.unknown(),
            ExpressionKind::Literal(literal) => self.literal_ty(literal),
            ExpressionKind::NameRef(_) => self.infer_read(id),
            ExpressionKind::Assign {
                spelling: _,
                target,
                value,
            } => {
                let value_ty = self.infer(*value);
                self.write_target(*target, value_ty);
                value_ty
            }
            ExpressionKind::Unary { operator, operand } => {
                let operand_ty = self.infer(*operand);
                self.infer_unary(range, *operator, operand_ty)
            }
            ExpressionKind::Binary { operator, lhs, rhs } => {
                let lhs_ty = self.infer(*lhs);
                let rhs_ty = self.infer(*rhs);
                self.infer_binary(range, *operator, lhs_ty, rhs_ty)
            }
            ExpressionKind::Call { callee, arguments } => {
                let arguments = arguments.clone();
                self.infer_call_expression(range, *callee, &arguments)
            }
            // Indexing and field access type as Unknown until the container
            // rules land (tuple/record projection, vector element rules).
            ExpressionKind::Index {
                target, arguments, ..
            } => {
                self.infer(*target);
                for argument in arguments {
                    if let Some(value) = argument.value {
                        self.infer(value);
                    }
                }
                self.unknown()
            }
            ExpressionKind::Field { target, .. } => {
                self.infer(*target);
                self.unknown()
            }
            ExpressionKind::Namespace { .. } => self.unknown(),
            // R parameters are always matchable by name and by position, so
            // inferred function types carry every formal as a named parameter
            // (optional when it defaults); a `...` formal becomes a rest
            // parameter with element `Any` at its formal position. Defaults
            // are inferred but do not pin an unannotated parameter's type —
            // that comes from the parameter's uses.
            ExpressionKind::Function { parameters, body } => {
                let parameters = parameters.clone();
                self.table.level += 1;
                let mark = self.environment.mark();
                let mut named = Vec::new();
                let mut variadic = None;
                for parameter in &parameters {
                    if parameter.name == "..." {
                        variadic = Some(crate::types::RestParameter {
                            element: crate::types::any(self.db),
                            preceding_named: named.len(),
                        });
                        continue;
                    }
                    let parameter_ty = self.fresh(Constraint::Unconstrained);
                    named.push(crate::types::RecordField {
                        name: Name::new(self.db, parameter.name.clone()),
                        ty: parameter_ty,
                        optional: parameter.default.is_some(),
                    });
                    if let Some(slot) = self
                        .naming
                        .bindings
                        .iter()
                        .find(|(_, info)| info.range == parameter.range)
                        .map(|(id, _)| *id)
                    {
                        self.environment.set(slot, EnvEntry::Mono(parameter_ty));
                    }
                    if let Some(default) = parameter.default {
                        self.infer(default);
                    }
                }
                let return_ty = self.infer(*body);
                self.environment.rollback(mark);
                self.table.level -= 1;
                Ty::new(
                    self.db,
                    TyKind::Function(FunctionType {
                        positional: Vec::new(),
                        named,
                        variadic,
                        ret: return_ty,
                    }),
                )
            }
            ExpressionKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.infer(*condition);
                let mark = self.environment.mark();
                let then_ty = self.infer(*then_branch);
                let then_writes = self.environment.writes_since(mark);
                self.environment.rollback(mark);
                let else_ty = match else_branch {
                    Some(else_branch) => self.infer(*else_branch),
                    None => crate::types::null(self.db),
                };
                self.join_writes(then_writes);
                self.join_types(then_ty, else_ty)
            }
            ExpressionKind::For { sequence, body, .. } => {
                self.infer(*sequence);
                let mark = self.environment.mark();
                self.infer(*body);
                let writes = self.environment.writes_since(mark);
                self.environment.rollback(mark);
                self.join_writes(writes);
                crate::types::null(self.db)
            }
            ExpressionKind::While { condition, body } => {
                self.infer(*condition);
                let mark = self.environment.mark();
                self.infer(*body);
                let writes = self.environment.writes_since(mark);
                self.environment.rollback(mark);
                self.join_writes(writes);
                crate::types::null(self.db)
            }
            ExpressionKind::Repeat { body } => {
                self.infer(*body);
                crate::types::null(self.db)
            }
            ExpressionKind::Block(statements) => {
                let statements = statements.clone();
                let mut last = crate::types::null(self.db);
                for statement in statements {
                    last = self.infer(statement);
                }
                last
            }
            ExpressionKind::Paren(inner) => self.infer(*inner),
            ExpressionKind::Break | ExpressionKind::Next => self.unknown(),
        };
        self.record(id, ty)
    }

    fn literal_ty(&mut self, literal: &LiteralKind) -> Ty<'db> {
        match literal {
            LiteralKind::Integer => scalar(self.db, Atomic::Integer),
            LiteralKind::Double { .. } => scalar(self.db, Atomic::Double),
            LiteralKind::Complex => scalar(self.db, Atomic::Complex),
            LiteralKind::String(_) => scalar(self.db, Atomic::Character),
            LiteralKind::Logical(_) => scalar(self.db, Atomic::Logical),
            LiteralKind::Null => crate::types::null(self.db),
            LiteralKind::Na => scalar(self.db, Atomic::Logical),
            LiteralKind::Inf | LiteralKind::NaN => scalar(self.db, Atomic::Double),
        }
    }

    fn infer_read(&mut self, id: ExprId) -> Ty<'db> {
        let Some(&slot) = self.naming.resolutions.get(&id) else {
            // A non-local read: resolve through the package interface (and, in
            // a later slice, the stub corpus). Unresolved reads stay silent
            // Unknown; naming owns the could-not-resolve diagnostic.
            if let Some(name) = self.naming.non_locals.get(&id)
                && let Some(scheme) = self.globals.and_then(|globals| globals.scheme(name))
            {
                return self.instantiate(&scheme);
            }
            return self.unknown();
        };
        match self.environment.get(slot) {
            Some(EnvEntry::Mono(ty)) => ty,
            Some(EnvEntry::Scheme(index)) => {
                let scheme = self.schemes()[index as usize].clone();
                self.instantiate(&scheme)
            }
            // Forward/recursive read before any write: tolerate as Unknown.
            None => self.unknown(),
        }
    }

    fn write_target(&mut self, target: ExprId, value_ty: Ty<'db>) {
        let target_expression = self.module.expression(target).clone();
        if let ExpressionKind::NameRef(_) = target_expression.kind {
            if let Some(&slot) = self.naming.resolutions.get(&target) {
                // Function values generalize at the binding (let-polymorphism);
                // everything else stays a monotype slot write.
                let resolved = self.table.shallow_resolve(self.db, value_ty);
                if matches!(resolved.kind(self.db), TyKind::Function(_)) {
                    let scheme = self.generalize(value_ty);
                    let index = self.push_scheme(scheme);
                    self.environment.set(slot, EnvEntry::Scheme(index));
                } else {
                    self.environment.set(slot, EnvEntry::Mono(value_ty));
                }
            }
            self.recorded.insert(target, value_ty);
        }
        // Replacement targets (`names(x) <- v`) were already inferred as reads
        // by naming; their typing rules land with the container rules.
    }

    fn infer_unary(
        &mut self,
        range: TextRange,
        operator: UnaryOperator,
        operand: Ty<'db>,
    ) -> Ty<'db> {
        match operator {
            UnaryOperator::Minus | UnaryOperator::Plus => {
                let numeric = self.fresh(Constraint::Numeric);
                self.unify_or_report(range, numeric, operand);
                numeric
            }
            UnaryOperator::Not => scalar(self.db, Atomic::Logical),
            UnaryOperator::Tilde | UnaryOperator::Help => self.unknown(),
        }
    }

    fn infer_binary(
        &mut self,
        range: TextRange,
        operator: BinaryOperator,
        lhs: Ty<'db>,
        rhs: Ty<'db>,
    ) -> Ty<'db> {
        use BinaryOperator::*;
        match operator {
            Add | Subtract | Multiply | Divide | Power | Modulo | IntegerDivide => {
                self.arithmetic(range, lhs, rhs)
            }
            Sequence => {
                let element = self.arithmetic(range, lhs, rhs);
                Ty::new(self.db, TyKind::Vector(element))
            }
            Less | Greater | LessEq | GreaterEq | Equal | NotEqual => {
                scalar(self.db, Atomic::Logical)
            }
            And | And2 | Or | Or2 => scalar(self.db, Atomic::Logical),
            Special => self.unknown(),
            Pipe => {
                // `x |> f()` types as the call result; the call already
                // inferred with the piped value prepended in a later slice —
                // Unknown until then.
                let _ = (lhs, rhs);
                self.unknown()
            }
            Tilde | Help => self.unknown(),
        }
    }

    /// Check a function definition against its declared annotation type:
    /// formals get their declared types (name-aware: named declarations match
    /// by name, positional declarations fill the rest in order; rigid binder
    /// types refuse to bind), the body infers under them, and the result
    /// checks against the declared return.
    fn check_declared_function(&mut self, function_id: ExprId, declared: &FunctionType<'db>) {
        let expression = self.module.expression(function_id).clone();
        let ExpressionKind::Function { parameters, body } = &expression.kind else {
            return;
        };
        let range = expression.range;

        self.table.level += 1;
        let mark = self.environment.mark();
        let mut positional_index = 0usize;
        let mut used_named: Vec<bool> = vec![false; declared.named.len()];
        for parameter in parameters {
            let declared_ty = if let Some(index) = declared
                .named
                .iter()
                .position(|field| field.name.text(self.db) == parameter.name)
            {
                used_named[index] = true;
                Some(declared.named[index].ty)
            } else if parameter.name == "..." {
                None
            } else if positional_index < declared.positional.len() {
                let ty = declared.positional[positional_index];
                positional_index += 1;
                Some(ty)
            } else if declared.variadic.is_some() {
                None
            } else {
                self.errors.push(TypeError {
                    range: parameter.range,
                    kind: TypeErrorKind::AnnotationParameterMismatch {
                        name: parameter.name.clone(),
                    },
                });
                None
            };
            let declared = declared_ty.is_some();
            let parameter_ty = declared_ty.unwrap_or_else(|| self.fresh(Constraint::Unconstrained));
            if let Some(slot) = self
                .naming
                .bindings
                .iter()
                .find(|(_, info)| info.range == parameter.range)
                .map(|(id, _)| *id)
            {
                self.environment.set(slot, EnvEntry::Mono(parameter_ty));
            }
            if let Some(default) = parameter.default {
                let default_ty = self.infer(default);
                // A `NULL` default is R's "no value" sentinel for optional
                // parameters, always allowed regardless of the declared type;
                // any other default must fit the declared type. An undeclared
                // formal's type comes from its uses, not its default.
                let resolved_default = self.table.resolve(self.db, default_ty);
                if declared && !matches!(resolved_default.kind(self.db), TyKind::Null) {
                    let whole_double = self.is_whole_double(default);
                    let default_range = self.module.expression(default).range;
                    if let Err(error) =
                        self.check_argument(parameter_ty, default_ty, default_range, whole_double)
                    {
                        self.errors.push(error);
                    }
                }
            }
        }
        // Declared named parameters the definition never declares.
        for (index, field) in declared.named.iter().enumerate() {
            if !used_named[index]
                && !parameters
                    .iter()
                    .any(|p| p.name == field.name.text(self.db))
            {
                self.errors.push(TypeError {
                    range,
                    kind: TypeErrorKind::AnnotationParameterMismatch {
                        name: field.name.text(self.db).to_owned(),
                    },
                });
            }
        }

        let body_ty = self.infer(*body);
        // An Unknown declared return (elided `->`) constrains nothing.
        if !matches!(declared.ret.kind(self.db), TyKind::Unknown) {
            let body_range = self.module.expression(*body).range;
            self.unify_or_report(body_range, declared.ret, body_ty);
        }
        self.environment.rollback(mark);
        self.table.level -= 1;
    }

    /// Arithmetic: each operand is *checked* numeric — a variable operand
    /// gains the numeric constraint without binding to the other side (so
    /// `function(x) x + 1L` stays `<T: numeric> fn(x: T) -> T`); two variable
    /// operands unify with each other; two concrete scalars promote
    /// (integer ∘ double = double).
    fn arithmetic(&mut self, range: TextRange, lhs: Ty<'db>, rhs: Ty<'db>) -> Ty<'db> {
        let lhs = self.table.shallow_resolve(self.db, lhs);
        let rhs = self.table.shallow_resolve(self.db, rhs);
        let constrain = |checker: &mut Self, side: Ty<'db>| match side.kind(checker.db) {
            TyKind::Var(_) => {
                let numeric = checker.fresh(Constraint::Numeric);
                checker.unify_or_report(range, numeric, side);
                true
            }
            // A rigid binder satisfies arithmetic only when its declaration
            // promised numeric-ness; the body must not add bounds the
            // annotation never declared.
            TyKind::Rigid(name)
                if matches!(
                    checker.rigid_constraints.get(name),
                    Some(Constraint::Numeric | Constraint::ScalarNumeric)
                ) =>
            {
                true
            }
            TyKind::Scalar(Atomic::Integer | Atomic::Double | Atomic::Complex)
            | TyKind::Any
            | TyKind::Unknown => true,
            _ => {
                let expected = checker.fresh(Constraint::Numeric);
                checker.errors.push(TypeError {
                    range,
                    kind: TypeErrorKind::Mismatch {
                        expected,
                        found: checker.table.resolve(checker.db, side),
                    },
                });
                false
            }
        };
        let lhs_ok = constrain(self, lhs);
        let rhs_ok = constrain(self, rhs);
        if !lhs_ok || !rhs_ok {
            return self.unknown();
        }
        match (lhs.kind(self.db), rhs.kind(self.db)) {
            (TyKind::Var(_), TyKind::Var(_)) => {
                self.unify_or_report(range, lhs, rhs);
                lhs
            }
            (TyKind::Var(_), _) => lhs,
            (_, TyKind::Var(_)) => rhs,
            (TyKind::Rigid(_), _) => lhs,
            (_, TyKind::Rigid(_)) => rhs,
            (TyKind::Any, _) | (_, TyKind::Any) => crate::types::any(self.db),
            (TyKind::Unknown, _) | (_, TyKind::Unknown) => self.unknown(),
            (TyKind::Scalar(a), TyKind::Scalar(b)) => {
                let promoted = match (a, b) {
                    (Atomic::Complex, _) | (_, Atomic::Complex) => Atomic::Complex,
                    (Atomic::Double, _) | (_, Atomic::Double) => Atomic::Double,
                    _ => Atomic::Integer,
                };
                scalar(self.db, promoted)
            }
            _ => self.unknown(),
        }
    }

    /// The call entry point: an overloaded stub callee resolves per call site
    /// (each candidate probed in declaration order); everything else infers
    /// the callee and dispatches on its type.
    fn infer_call_expression(
        &mut self,
        range: TextRange,
        callee: ExprId,
        arguments: &[Argument],
    ) -> Ty<'db> {
        if let Some(ty) = self.try_overloaded_call(range, callee, arguments) {
            return ty;
        }
        let callee_ty = self.infer(callee);
        let call_arguments = self.infer_call_arguments(range, arguments);
        self.dispatch_call(range, callee_ty, &call_arguments)
    }

    /// Ordered overload probing: the first candidate whose signature accepts
    /// the arguments wins and its return is the call's type. Only a plain
    /// name resolves through an overload set, and a local binding shadowing
    /// the name disables it (the local wins, as everywhere). `None` means
    /// "not an overloaded call" — fall through to normal dispatch.
    fn try_overloaded_call(
        &mut self,
        range: TextRange,
        callee: ExprId,
        arguments: &[Argument],
    ) -> Option<Ty<'db>> {
        let ExpressionKind::NameRef(name) = &self.module.expression(callee).kind else {
            return None;
        };
        let name = name.clone();
        if self.naming.resolutions.contains_key(&callee) {
            return None;
        }
        let schemes = self.globals?.overloads(&name)?;
        if schemes.len() < 2 {
            return None;
        }

        // Arguments are inferred exactly once, before any probe: expression
        // inference writes state the probe snapshot does not reverse (the
        // environment, recorded expression types), so running it inside a
        // probe would leak bindings that reference rolled-back variable ids.
        let call_arguments = self.infer_call_arguments(range, arguments);

        // Selection needs concrete argument types. Probing against an
        // argument whose type still contains a free inference variable would
        // let the first candidate bind it — committing a wrapper function's
        // parameter (`function(x) sum(x)`) to the first candidate's parameter
        // type and rejecting calls R accepts. Such a call skips selection and
        // uses the final declaration, by corpus convention the most general.
        let has_unresolved_argument = call_arguments.iter().any(|argument| {
            argument
                .ty
                .is_some_and(|ty| self.table.contains_unbound_var(self.db, ty))
        });
        let probed: Vec<TypeScheme<'db>> = if has_unresolved_argument {
            vec![schemes.last().cloned()?]
        } else {
            schemes
        };

        // Selection runs strict first, then (only if nothing matched and a
        // whole-number double literal is present) once more with the
        // literal-as-integer courtesy: `1` is genuinely a double at runtime,
        // so letting it match an integer candidate in the strict round would
        // pick a signature whose return misstates what R computes
        // (`sum(1, 2)` is a double). The courtesy round keeps a name whose
        // only fitting candidate wants `integer` callable as `foo(1)`.
        let rounds: &[bool] = if call_arguments.iter().any(|argument| argument.whole_double) {
            &[false, true]
        } else {
            &[false]
        };

        let mut first_error: Option<TypeError<'db>> = None;
        for &courtesy in rounds {
            for scheme in &probed {
                let snapshot = self.table.snapshot();
                let instantiated = self.instantiate(scheme);
                let resolved = self.table.shallow_resolve(self.db, instantiated);
                let TyKind::Function(function) = resolved.kind(self.db).clone() else {
                    self.table.rollback(snapshot);
                    continue;
                };
                if !courtesy {
                    self.overload_probe_depth += 1;
                }
                let outcome = self.match_arguments(range, &function, &call_arguments);
                if !courtesy {
                    self.overload_probe_depth -= 1;
                }
                match outcome {
                    Ok(()) => {
                        self.recorded.insert(callee, resolved);
                        return Some(function.ret);
                    }
                    Err(error) => {
                        self.table.rollback(snapshot);
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
        }

        // The unresolved-argument fallback probes a single candidate; failing
        // it is an ordinary call mismatch, so the underlying error reads
        // better than a one-candidate overload report.
        let error = match (probed.len(), first_error) {
            (1, Some(error)) => error,
            (candidates, first) => TypeError {
                range,
                kind: TypeErrorKind::NoMatchingOverload {
                    name,
                    candidates,
                    first: first.map(Box::new),
                },
            },
        };
        self.errors.push(error);
        self.recorded.insert(callee, self.unknown());
        Some(self.unknown())
    }

    fn infer_call_arguments(
        &mut self,
        range: TextRange,
        arguments: &[Argument],
    ) -> Vec<CallArgument<'db>> {
        arguments
            .iter()
            .map(|argument| {
                let ty = argument.value.map(|value| self.infer(value));
                let argument_range = argument
                    .value
                    .map(|value| self.module.expression(value).range)
                    .unwrap_or(range);
                let whole_double = argument
                    .value
                    .is_some_and(|value| self.is_whole_double(value));
                CallArgument {
                    name: argument.name.clone(),
                    ty,
                    range: argument_range,
                    whole_double,
                }
            })
            .collect()
    }

    fn is_whole_double(&self, value: ExprId) -> bool {
        matches!(
            self.module.expression(value).kind,
            ExpressionKind::Literal(LiteralKind::Double { whole_number: true })
        )
    }

    fn dispatch_call(
        &mut self,
        range: TextRange,
        callee: Ty<'db>,
        arguments: &[CallArgument<'db>],
    ) -> Ty<'db> {
        let resolved = self.table.shallow_resolve(self.db, callee);
        match resolved.kind(self.db) {
            TyKind::Function(function) => {
                let function = function.clone();
                if let Err(error) = self.match_arguments(range, &function, arguments) {
                    self.errors.push(error);
                }
                function.ret
            }
            TyKind::Any | TyKind::Unknown => self.unknown(),
            TyKind::Var(_) => {
                // An unresolved callee: constrain it to a function of the
                // observed shape.
                let ret = self.fresh(Constraint::Unconstrained);
                let positional: Vec<Ty<'db>> = arguments
                    .iter()
                    .filter(|argument| argument.name.is_none())
                    .map(|argument| argument.ty.unwrap_or_else(|| self.unknown()))
                    .collect();
                let expected = Ty::new(
                    self.db,
                    TyKind::Function(FunctionType {
                        positional,
                        named: arguments
                            .iter()
                            .filter_map(|argument| {
                                argument
                                    .name
                                    .as_ref()
                                    .map(|name| crate::types::RecordField {
                                        name: Name::new(self.db, name.clone()),
                                        ty: argument.ty.unwrap_or_else(|| self.unknown()),
                                        optional: false,
                                    })
                            })
                            .collect(),
                        variadic: None,
                        ret,
                    }),
                );
                self.unify_or_report(range, expected, resolved);
                ret
            }
            // A call through a union of functions — the dispatch-table idiom,
            // `handlers[[name]](...)` — must be valid for every member, since
            // the value could be any of them. Each member's signature is
            // probed against the arguments in an isolated snapshot and the
            // call's type is the union of the member returns; returns are
            // variable-erased because the probe bindings that produced them
            // roll back.
            TyKind::Union(members)
                if members.iter().all(|&member| {
                    matches!(
                        self.table.shallow_resolve(self.db, member).kind(self.db),
                        TyKind::Function(_)
                    )
                }) =>
            {
                let members = members.clone();
                let mut returns = Vec::with_capacity(members.len());
                for member in members {
                    let member = self.table.shallow_resolve(self.db, member);
                    let TyKind::Function(function) = member.kind(self.db).clone() else {
                        continue;
                    };
                    let snapshot = self.table.snapshot();
                    match self.match_arguments(range, &function, arguments) {
                        Ok(()) => {
                            let member_return = self.table.resolve(self.db, function.ret);
                            self.table.rollback(snapshot);
                            returns.push(crate::types::erase_vars(self.db, member_return));
                        }
                        Err(error) => {
                            self.table.rollback(snapshot);
                            self.errors.push(error);
                            return self.unknown();
                        }
                    }
                }
                union_of(self.db, returns)
            }
            _ => {
                self.errors.push(TypeError {
                    range,
                    kind: TypeErrorKind::NotAFunction {
                        found: self.table.resolve(self.db, resolved),
                    },
                });
                self.unknown()
            }
        }
    }

    /// R's argument matcher over already-inferred argument types: named
    /// arguments consume their same-named formal; positionals fill the fixed
    /// positional parameters, then the named formals declared before the rest
    /// parameter (all of them when the function is not variadic), then the
    /// rest parameter absorbs the overflow. The first failing argument aborts
    /// the match, so an overload probe can run this inside a snapshot.
    fn match_arguments(
        &mut self,
        range: TextRange,
        function: &FunctionType<'db>,
        arguments: &[CallArgument<'db>],
    ) -> Result<(), TypeError<'db>> {
        let total = function.positional.len() + function.named.len();
        let required = function.positional.len()
            + function
                .named
                .iter()
                .filter(|field| !field.optional)
                .count();
        let variadic_element = function.variadic.as_ref().map(|rest| rest.element);
        // Named parameters declared before the rest parameter fill
        // positionally, exactly as R fills formals before `...`. Removals
        // keep declaration order, so the pre-rest parameters are always the
        // front segment of the remaining list and this count tracks them.
        let mut pre_rest_remaining = match &function.variadic {
            Some(rest) => rest.preceding_named,
            None => function.named.len(),
        };
        let mut remaining_named = function.named.clone();
        let mut next_positional = 0usize;

        for argument in arguments {
            match &argument.name {
                Some(name) => {
                    let position = remaining_named
                        .iter()
                        .position(|field| field.name.text(self.db) == name.as_str());
                    match position {
                        Some(index) => {
                            let field = remaining_named.remove(index);
                            if index < pre_rest_remaining {
                                pre_rest_remaining -= 1;
                            }
                            if let Some(ty) = argument.ty {
                                self.check_argument(
                                    field.ty,
                                    ty,
                                    argument.range,
                                    argument.whole_double,
                                )?;
                            }
                        }
                        None => {
                            // A named argument matching no declared parameter
                            // is absorbed by the rest parameter (R collects
                            // unmatched keywords into `...`); a name that
                            // *duplicates* an already-given declared parameter
                            // stays an error, as does an unmatched name on a
                            // non-variadic function.
                            let duplicates_declared = function
                                .named
                                .iter()
                                .any(|field| field.name.text(self.db) == name.as_str());
                            match (variadic_element, duplicates_declared) {
                                (Some(element), false) => {
                                    if let Some(ty) = argument.ty {
                                        self.check_argument(
                                            element,
                                            ty,
                                            argument.range,
                                            argument.whole_double,
                                        )?;
                                    }
                                }
                                _ => {
                                    return Err(TypeError {
                                        range: argument.range,
                                        kind: TypeErrorKind::UnknownArgument { name: name.clone() },
                                    });
                                }
                            }
                        }
                    }
                }
                None => {
                    if next_positional < function.positional.len() {
                        let expected = function.positional[next_positional];
                        next_positional += 1;
                        if let Some(ty) = argument.ty {
                            self.check_argument(
                                expected,
                                ty,
                                argument.range,
                                argument.whole_double,
                            )?;
                        }
                    } else if pre_rest_remaining > 0 {
                        let field = remaining_named.remove(0);
                        pre_rest_remaining -= 1;
                        if let Some(ty) = argument.ty {
                            self.check_argument(
                                field.ty,
                                ty,
                                argument.range,
                                argument.whole_double,
                            )?;
                        }
                    } else if let Some(element) = variadic_element {
                        if let Some(ty) = argument.ty {
                            self.check_argument(
                                element,
                                ty,
                                argument.range,
                                argument.whole_double,
                            )?;
                        }
                    } else {
                        return Err(TypeError {
                            range,
                            kind: TypeErrorKind::ArityMismatch {
                                expected: total,
                                found: arguments.len(),
                            },
                        });
                    }
                }
            }
        }

        if next_positional != function.positional.len()
            || remaining_named.iter().any(|field| !field.optional)
        {
            return Err(TypeError {
                range,
                kind: TypeErrorKind::ArityMismatch {
                    expected: required,
                    found: arguments.len(),
                },
            });
        }
        Ok(())
    }

    /// One argument against one parameter type: compatibility, not
    /// unification, so parameter-position coercions (scalar-to-vector, `T`
    /// into `T | NULL`, integer widening) apply. An `Unknown` argument is
    /// accepted to avoid cascading a second error after the cause was already
    /// diagnosed where the value became `Unknown`.
    fn check_argument(
        &mut self,
        expected: Ty<'db>,
        found: Ty<'db>,
        range: TextRange,
        whole_double: bool,
    ) -> Result<(), TypeError<'db>> {
        let resolved_found = self.table.resolve(self.db, found);
        if matches!(resolved_found.kind(self.db), TyKind::Unknown) {
            return Ok(());
        }
        if self.table.compatible(self.db, resolved_found, expected) {
            return Ok(());
        }
        // R programmers write `seq_len(10)`, not `seq_len(10L)`: a
        // whole-number double literal counts as an integer at a parameter
        // position. The retry goes through full compatibility, so
        // integer-expecting unions and vector parameters admit the literal
        // too. Off during a strict overload probe — the courtesy must not
        // decide which candidate wins.
        if self.overload_probe_depth == 0
            && matches!(resolved_found.kind(self.db), TyKind::Scalar(Atomic::Double))
            && whole_double
            && self
                .table
                .compatible(self.db, scalar(self.db, Atomic::Integer), expected)
        {
            return Ok(());
        }
        // A numeric-constrained parameter rejected the argument because it is
        // not numeric; report that directly rather than rendering the bare
        // inference variable as the expected type.
        let resolved_expected = self.table.resolve(self.db, expected);
        if let TyKind::Var(var) = resolved_expected.kind(self.db)
            && let Entry::Unbound {
                constraint: Constraint::Numeric,
                ..
            } = self.table.entry(*var)
        {
            return Err(TypeError {
                range,
                kind: TypeErrorKind::ConstraintViolation {
                    constraint: Constraint::Numeric,
                    found: resolved_found,
                },
            });
        }
        Err(TypeError {
            range,
            kind: TypeErrorKind::Mismatch {
                expected: resolved_expected,
                found: resolved_found,
            },
        })
    }

    /// Branch-merge join: unify when possible (keeps the chooser idiom linking
    /// two inference variables), otherwise the union of the branch types; a
    /// NULL branch joins by pure union so it never binds a variable to NULL.
    fn join_types(&mut self, left: Ty<'db>, right: Ty<'db>) -> Ty<'db> {
        let left_resolved = self.table.resolve(self.db, left);
        let right_resolved = self.table.resolve(self.db, right);
        if matches!(left_resolved.kind(self.db), TyKind::Null)
            || matches!(right_resolved.kind(self.db), TyKind::Null)
        {
            return union_of(self.db, [left_resolved, right_resolved]);
        }
        let snapshot = self.table.snapshot();
        match self.table.unify(self.db, left, right) {
            Ok(()) => left,
            Err(_) => {
                self.table.rollback(snapshot);
                union_of(self.db, [left_resolved, right_resolved])
            }
        }
    }

    /// Merge branch writes with the pre-branch state: an entry written in one
    /// branch joins with its other-path value (or stays, optimistically, when
    /// the other path had none).
    fn join_writes(&mut self, writes: Vec<(BindingId, Option<EnvEntry<'db>>)>) {
        for (slot, branch_entry) in writes {
            let current = self.environment.get(slot);
            let joined = match (current, branch_entry) {
                (Some(EnvEntry::Mono(a)), Some(EnvEntry::Mono(b))) if a != b => {
                    Some(EnvEntry::Mono(self.join_types(a, b)))
                }
                (None, Some(entry)) => Some(entry),
                (_, entry @ Some(EnvEntry::Scheme(_))) => entry,
                (Some(_), Some(entry)) => Some(entry),
                (_, None) => None,
            };
            if let Some(entry) = joined {
                self.environment.set(slot, entry);
            }
        }
    }

    // ---- schemes ----

    fn schemes(&self) -> &Vec<TypeScheme<'db>> {
        &self.scheme_arena
    }

    fn push_scheme(&mut self, scheme: TypeScheme<'db>) -> u32 {
        self.scheme_arena.push(scheme);
        (self.scheme_arena.len() - 1) as u32
    }

    /// Generalize: quantify unbound variables ABOVE the current level.
    fn generalize(&mut self, ty: Ty<'db>) -> TypeScheme<'db> {
        let resolved = self.table.resolve(self.db, ty);
        let mut binders = Vec::new();
        let mut mapping: FxHashMap<crate::types::InferenceVar, Name<'db>> = FxHashMap::default();
        let body = self.abstract_vars(resolved, &mut binders, &mut mapping);
        TypeScheme { binders, body }
    }

    fn abstract_vars(
        &mut self,
        ty: Ty<'db>,
        binders: &mut Vec<(Name<'db>, Constraint)>,
        mapping: &mut FxHashMap<crate::types::InferenceVar, Name<'db>>,
    ) -> Ty<'db> {
        match ty.kind(self.db).clone() {
            TyKind::Var(var) => {
                let representative = self.table.find(var);
                if let Some(&name) = mapping.get(&representative) {
                    return Ty::new(self.db, TyKind::Rigid(name));
                }
                let Entry::Unbound { level, constraint } = *self.table.entry(representative) else {
                    return ty;
                };
                if level <= self.table.level {
                    // Escaped below the binding level: stays monomorphic.
                    return ty;
                }
                let letter = (b'T' + (binders.len() as u8 % 7)) as char;
                let suffix = binders.len() / 7;
                let text = if suffix == 0 {
                    letter.to_string()
                } else {
                    format!("{letter}{suffix}")
                };
                let name = Name::new(self.db, text);
                mapping.insert(representative, name);
                binders.push((name, constraint));
                Ty::new(self.db, TyKind::Rigid(name))
            }
            TyKind::Vector(inner) => {
                let inner = self.abstract_vars(inner, binders, mapping);
                Ty::new(self.db, TyKind::Vector(inner))
            }
            TyKind::NamedVector(inner) => {
                let inner = self.abstract_vars(inner, binders, mapping);
                Ty::new(self.db, TyKind::NamedVector(inner))
            }
            TyKind::List(inner) => {
                let inner = self.abstract_vars(inner, binders, mapping);
                Ty::new(self.db, TyKind::List(inner))
            }
            TyKind::NamedList(inner) => {
                let inner = self.abstract_vars(inner, binders, mapping);
                Ty::new(self.db, TyKind::NamedList(inner))
            }
            TyKind::Tuple(items) => {
                let items = items
                    .iter()
                    .map(|&item| self.abstract_vars(item, binders, mapping))
                    .collect();
                Ty::new(self.db, TyKind::Tuple(items))
            }
            TyKind::Record(fields) => {
                let fields = fields
                    .iter()
                    .map(|field| {
                        let mut field = field.clone();
                        field.ty = self.abstract_vars(field.ty, binders, mapping);
                        field
                    })
                    .collect();
                Ty::new(self.db, TyKind::Record(fields))
            }
            TyKind::Function(function) => {
                let function = FunctionType {
                    positional: function
                        .positional
                        .iter()
                        .map(|&ty| self.abstract_vars(ty, binders, mapping))
                        .collect(),
                    named: function
                        .named
                        .iter()
                        .map(|field| {
                            let mut field = field.clone();
                            field.ty = self.abstract_vars(field.ty, binders, mapping);
                            field
                        })
                        .collect(),
                    variadic: function.variadic.as_ref().map(|rest| {
                        let mut rest = rest.clone();
                        rest.element = self.abstract_vars(rest.element, binders, mapping);
                        rest
                    }),
                    ret: self.abstract_vars(function.ret, binders, mapping),
                };
                Ty::new(self.db, TyKind::Function(function))
            }
            TyKind::Union(members) => {
                let members: Vec<Ty<'db>> = members
                    .iter()
                    .map(|&member| self.abstract_vars(member, binders, mapping))
                    .collect();
                union_of(self.db, members)
            }
            TyKind::Named(name, arguments) => {
                let arguments = arguments
                    .iter()
                    .map(|&argument| self.abstract_vars(argument, binders, mapping))
                    .collect();
                Ty::new(self.db, TyKind::Named(name, arguments))
            }
            _ => ty,
        }
    }

    /// Instantiate a scheme with fresh variables carrying its constraints.
    fn instantiate(&mut self, scheme: &TypeScheme<'db>) -> Ty<'db> {
        let mut substitution: FxHashMap<Name<'db>, Ty<'db>> = FxHashMap::default();
        for (name, constraint) in &scheme.binders {
            let fresh = self.fresh(*constraint);
            substitution.insert(*name, fresh);
        }
        self.substitute_rigid(scheme.body, &substitution)
    }

    fn substitute_rigid(
        &mut self,
        ty: Ty<'db>,
        substitution: &FxHashMap<Name<'db>, Ty<'db>>,
    ) -> Ty<'db> {
        match ty.kind(self.db).clone() {
            TyKind::Rigid(name) => substitution.get(&name).copied().unwrap_or(ty),
            TyKind::Vector(inner) => {
                let inner = self.substitute_rigid(inner, substitution);
                Ty::new(self.db, TyKind::Vector(inner))
            }
            TyKind::NamedVector(inner) => {
                let inner = self.substitute_rigid(inner, substitution);
                Ty::new(self.db, TyKind::NamedVector(inner))
            }
            TyKind::List(inner) => {
                let inner = self.substitute_rigid(inner, substitution);
                Ty::new(self.db, TyKind::List(inner))
            }
            TyKind::NamedList(inner) => {
                let inner = self.substitute_rigid(inner, substitution);
                Ty::new(self.db, TyKind::NamedList(inner))
            }
            TyKind::Tuple(items) => {
                let items = items
                    .iter()
                    .map(|&item| self.substitute_rigid(item, substitution))
                    .collect();
                Ty::new(self.db, TyKind::Tuple(items))
            }
            TyKind::Record(fields) => {
                let fields = fields
                    .iter()
                    .map(|field| {
                        let mut field = field.clone();
                        field.ty = self.substitute_rigid(field.ty, substitution);
                        field
                    })
                    .collect();
                Ty::new(self.db, TyKind::Record(fields))
            }
            TyKind::Function(function) => {
                let function = FunctionType {
                    positional: function
                        .positional
                        .iter()
                        .map(|&ty| self.substitute_rigid(ty, substitution))
                        .collect(),
                    named: function
                        .named
                        .iter()
                        .map(|field| {
                            let mut field = field.clone();
                            field.ty = self.substitute_rigid(field.ty, substitution);
                            field
                        })
                        .collect(),
                    variadic: function.variadic.as_ref().map(|rest| {
                        let mut rest = rest.clone();
                        rest.element = self.substitute_rigid(rest.element, substitution);
                        rest
                    }),
                    ret: self.substitute_rigid(function.ret, substitution),
                };
                Ty::new(self.db, TyKind::Function(function))
            }
            TyKind::Union(members) => {
                let members: Vec<Ty<'db>> = members
                    .iter()
                    .map(|&member| self.substitute_rigid(member, substitution))
                    .collect();
                union_of(self.db, members)
            }
            TyKind::Named(name, arguments) => {
                let arguments = arguments
                    .iter()
                    .map(|&argument| self.substitute_rigid(argument, substitution))
                    .collect();
                Ty::new(self.db, TyKind::Named(name, arguments))
            }
            _ => ty,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RootDatabase;
    use crate::hir::lower_item;
    use crate::naming::resolve_item;

    fn check_source<'db>(db: &'db RootDatabase, source: &str) -> ItemCheck<'db> {
        let parse = syntax::parse(source);
        let root = parse.syntax_node();
        let item = root.children().next().expect("one top-level item");
        let module = lower_item(&item);
        let naming = resolve_item(&module);
        check_item(db, &module, &naming)
    }

    #[test]
    fn arithmetic_generalizes_with_numeric_constraint() {
        let db = RootDatabase::default();
        let check = check_source(&db, "add <- function(x) x + 1L");
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        let scheme = check.scheme.expect("definition scheme");
        // `<T: numeric> fn(x: T) -> T`.
        assert_eq!(scheme.binders.len(), 1);
        assert_eq!(scheme.binders[0].1, Constraint::Numeric);
        let TyKind::Function(function) = scheme.body.kind(&db) else {
            panic!("expected a function scheme");
        };
        // Formals carry their names (R matches by name and position).
        assert_eq!(function.named.len(), 1);
        assert_eq!(function.named[0].name.text(&db), "x");
        assert_eq!(function.named[0].ty, function.ret);
    }

    #[test]
    fn call_mismatch_reports() {
        let db = RootDatabase::default();
        let check = check_source(
            &db,
            "g <- function() {\n  f <- function(x) x + 1\n  f(\"txt\")\n}",
        );
        assert!(
            check.errors.iter().any(|error| matches!(
                error.kind,
                TypeErrorKind::ConstraintViolation {
                    constraint: Constraint::Numeric,
                    ..
                }
            )),
            "expected a numeric-constraint violation, got {:?}",
            check.errors
        );
    }

    #[test]
    fn polymorphic_reuse_across_calls() {
        let db = RootDatabase::default();
        let check = check_source(
            &db,
            "use <- function() {\n  id <- function(x) x\n  id(1L)\n  id(\"s\")\n}",
        );
        assert!(
            check.errors.is_empty(),
            "let-polymorphism failed: {:?}",
            check.errors
        );
    }

    #[test]
    fn if_join_unifies_or_unions() {
        let db = RootDatabase::default();
        // NULL branch joins by union, never binding the variable.
        let check = check_source(&db, "pick <- function(f) if (f) 1L else NULL");
        assert!(check.errors.is_empty());
        let scheme = check.scheme.expect("scheme");
        let TyKind::Function(function) = scheme.body.kind(&db) else {
            panic!()
        };
        let TyKind::Union(members) = function.ret.kind(&db) else {
            panic!("expected integer | NULL, got {:?}", function.ret.kind(&db));
        };
        assert_eq!(members.len(), 2);
    }

    #[test]
    fn unknown_named_argument_reports() {
        let db = RootDatabase::default();
        let check = check_source(
            &db,
            "g <- function() {\n  f <- function(x) x\n  f(z = 1)\n}",
        );
        assert!(
            check
                .errors
                .iter()
                .any(|error| matches!(&error.kind, TypeErrorKind::UnknownArgument { name } if name == "z")),
            "expected unknown-argument, got {:?}",
            check.errors
        );
    }

    #[test]
    fn arity_mismatch_reports_both_directions() {
        let db = RootDatabase::default();
        let surplus = check_source(&db, "g <- function() {\n  f <- function(x) x\n  f(1, 2)\n}");
        assert!(
            surplus
                .errors
                .iter()
                .any(|error| matches!(error.kind, TypeErrorKind::ArityMismatch { .. })),
            "expected a surplus-argument arity error, got {:?}",
            surplus.errors
        );
        let missing = check_source(&db, "g <- function() {\n  f <- function(x) x\n  f()\n}");
        assert!(
            missing
                .errors
                .iter()
                .any(|error| matches!(error.kind, TypeErrorKind::ArityMismatch { .. })),
            "expected a missing-argument arity error, got {:?}",
            missing.errors
        );
        // A defaulted formal may stay unfilled.
        let defaulted = check_source(&db, "g <- function() {\n  f <- function(x = 1) x\n  f()\n}");
        assert!(defaulted.errors.is_empty(), "{:?}", defaulted.errors);
    }

    #[test]
    fn union_of_functions_calls_every_member() {
        let db = RootDatabase::default();
        // The dispatch-table idiom: the value could be either function, so
        // the call must be valid for both and types as the union of returns.
        let check = check_source(
            &db,
            "f <- function(flag) {\n  h <- if (flag) function(x) 1L else function(x) \"a\"\n  h(2L)\n}",
        );
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        let scheme = check.scheme.expect("scheme");
        let TyKind::Function(function) = scheme.body.kind(&db) else {
            panic!()
        };
        let TyKind::Union(members) = function.ret.kind(&db) else {
            panic!(
                "expected integer | character, got {:?}",
                function.ret.kind(&db)
            );
        };
        assert_eq!(members.len(), 2);
    }

    #[test]
    fn conditional_slot_write_joins_types() {
        let db = RootDatabase::default();
        // The beta-semantics soundness case: a branch write must be visible
        // after the construct as a union.
        let check = check_source(
            &db,
            "f <- function(flag) {\n  x <- 1L\n  if (flag) x <- \"two\"\n  x\n}",
        );
        assert!(check.errors.is_empty());
        let scheme = check.scheme.expect("scheme");
        let TyKind::Function(function) = scheme.body.kind(&db) else {
            panic!()
        };
        let TyKind::Union(members) = function.ret.kind(&db) else {
            panic!(
                "expected integer | character return, got {:?}",
                function.ret.kind(&db)
            );
        };
        assert_eq!(members.len(), 2);
    }

    fn check_annotated<'db>(db: &'db RootDatabase, source: &str) -> ItemCheck<'db> {
        let parse = syntax::parse(source);
        let root = parse.syntax_node();
        let annotation = root
            .children()
            .find(|child| child.kind() == syntax::SyntaxKind::ANNOTATION)
            .map(|node| crate::annotations::lower_annotation(db, &node));
        let item = root
            .children()
            .find(|child| syntax::ast::is_expression_kind(child.kind()))
            .expect("one top-level item");
        let module = lower_item(&item);
        let naming = resolve_item(&module);
        check_item_with_annotation(db, &module, &naming, annotation.as_ref(), None)
    }

    #[test]
    fn annotated_identity_keeps_declared_scheme() {
        let db = RootDatabase::default();
        let check = check_annotated(&db, "#: <T> fn(x: T) -> T\nid <- function(x) x");
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        let scheme = check.scheme.expect("declared scheme");
        assert_eq!(scheme.binders.len(), 1);
        let TyKind::Function(function) = scheme.body.kind(&db) else {
            panic!()
        };
        assert!(matches!(function.ret.kind(&db), TyKind::Rigid(_)));
    }

    #[test]
    fn annotated_return_mismatch_reports() {
        let db = RootDatabase::default();
        let check = check_annotated(&db, "#: fn(x: integer) -> character\nf <- function(x) x");
        assert!(
            check
                .errors
                .iter()
                .any(|e| matches!(e.kind, TypeErrorKind::Mismatch { .. })),
            "expected return mismatch, got {:?}",
            check.errors
        );
    }

    #[test]
    fn annotated_parameter_name_mismatch_reports() {
        let db = RootDatabase::default();
        let check = check_annotated(&db, "#: fn(x: integer) -> integer\nf <- function(y) 1L");
        assert!(
            check
                .errors
                .iter()
                .any(|e| matches!(&e.kind, TypeErrorKind::AnnotationParameterMismatch { .. })),
            "expected parameter mismatch, got {:?}",
            check.errors
        );
    }

    #[test]
    fn rigid_binder_refuses_undeclared_bound() {
        let db = RootDatabase::default();
        // The body adds an arithmetic bound the annotation never declared.
        let bad = check_annotated(&db, "#: <T> fn(x: T) -> T\nbad <- function(x) x + 1L");
        assert!(
            !bad.errors.is_empty(),
            "plain <T> must refuse arithmetic, got no errors"
        );
        // With the declared numeric constraint the same body is fine.
        let ok = check_annotated(
            &db,
            "#: <T: numeric> fn(x: T) -> T\nok <- function(x) x + 1L",
        );
        assert!(ok.errors.is_empty(), "{:?}", ok.errors);
    }

    #[test]
    fn expanded_param_return_form_checks() {
        let db = RootDatabase::default();
        let check = check_annotated(
            &db,
            "#: @forall T: numeric\n#: @param x {T}\n#: @return {T}\nround2 <- function(x) x + 1L",
        );
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        let scheme = check.scheme.expect("declared scheme");
        assert_eq!(scheme.binders.len(), 1);
        assert_eq!(scheme.binders[0].1, Constraint::Numeric);
    }

    #[test]
    fn trusted_annotation_skips_enforcement() {
        let db = RootDatabase::default();
        let check = check_annotated(
            &db,
            "#: @trust fn(x: integer) -> character\nf <- function(x) x",
        );
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        assert!(check.scheme.is_some());
    }
}
