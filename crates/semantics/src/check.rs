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
    TooManyArguments {
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

/// Resolver for names that are not item-local: package globals (and, in a
/// later slice, the stdlib stub corpus).
pub trait GlobalEnv<'db> {
    fn scheme(&self, name: &str) -> Option<TypeScheme<'db>>;
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
            UnifyError::ConstraintRejected(_, found) => TypeErrorKind::Mismatch {
                expected: self.fresh(Constraint::Numeric),
                found: self.table.resolve(self.db, found),
            },
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
                let callee_ty = self.infer(*callee);
                self.infer_call(range, callee_ty, arguments)
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
            ExpressionKind::Function { parameters, body } => {
                let parameters = parameters.clone();
                self.table.level += 1;
                let mark = self.environment.mark();
                let mut positional = Vec::new();
                for parameter in &parameters {
                    let parameter_ty = self.fresh(Constraint::Unconstrained);
                    positional.push(parameter_ty);
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
                        self.unify_or_report(range, positional[positional.len() - 1], default_ty);
                    }
                }
                let return_ty = self.infer(*body);
                self.environment.rollback(mark);
                self.table.level -= 1;
                Ty::new(
                    self.db,
                    TyKind::Function(FunctionType {
                        positional,
                        named: Vec::new(),
                        variadic: None,
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
            LiteralKind::Double => scalar(self.db, Atomic::Double),
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
                self.unify_or_report(parameter.range, parameter_ty, default_ty);
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

    fn infer_call(&mut self, range: TextRange, callee: Ty<'db>, arguments: &[Argument]) -> Ty<'db> {
        let arguments: Vec<(Option<String>, Option<Ty<'db>>, TextRange)> = arguments
            .iter()
            .map(|argument| {
                let ty = argument.value.map(|value| self.infer(value));
                let argument_range = argument
                    .value
                    .map(|value| self.module.expression(value).range)
                    .unwrap_or(range);
                (argument.name.clone(), ty, argument_range)
            })
            .collect();

        let resolved = self.table.shallow_resolve(self.db, callee);
        match resolved.kind(self.db) {
            TyKind::Function(function) => {
                let function = function.clone();
                self.match_arguments(range, &function, &arguments);
                function.ret
            }
            TyKind::Any | TyKind::Unknown => self.unknown(),
            TyKind::Var(_) => {
                // An unresolved callee: constrain it to a function of the
                // observed shape.
                let ret = self.fresh(Constraint::Unconstrained);
                let positional: Vec<Ty<'db>> = arguments
                    .iter()
                    .filter(|(name, _, _)| name.is_none())
                    .map(|(_, ty, _)| ty.unwrap_or_else(|| self.unknown()))
                    .collect();
                let expected = Ty::new(
                    self.db,
                    TyKind::Function(FunctionType {
                        positional,
                        named: arguments
                            .iter()
                            .filter_map(|(name, ty, _)| {
                                name.as_ref().map(|name| crate::types::RecordField {
                                    name: Name::new(self.db, name.clone()),
                                    ty: ty.unwrap_or_else(|| self.unknown()),
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

    /// Name-aware call matching: named arguments consume their formal; the
    /// rest fill positionally; a variadic absorbs the overflow.
    fn match_arguments(
        &mut self,
        range: TextRange,
        function: &FunctionType<'db>,
        arguments: &[(Option<String>, Option<Ty<'db>>, TextRange)],
    ) {
        let mut positional_index = 0usize;
        let mut named_consumed: Vec<bool> = vec![false; function.named.len()];
        for (name, ty, argument_range) in arguments {
            match name {
                Some(name) => {
                    let formal = function
                        .named
                        .iter()
                        .position(|field| field.name.text(self.db) == name.as_str());
                    match formal {
                        Some(index) => {
                            named_consumed[index] = true;
                            if let Some(ty) = ty {
                                let expected = function.named[index].ty;
                                self.unify_or_report(*argument_range, expected, *ty);
                            }
                        }
                        None => {
                            if function.variadic.is_none() {
                                self.errors.push(TypeError {
                                    range: *argument_range,
                                    kind: TypeErrorKind::UnknownArgument { name: name.clone() },
                                });
                            } else if let (Some(rest), Some(ty)) = (&function.variadic, ty) {
                                self.unify_or_report(*argument_range, rest.element, *ty);
                            }
                        }
                    }
                }
                None => {
                    if positional_index < function.positional.len() {
                        let expected = function.positional[positional_index];
                        positional_index += 1;
                        if let Some(ty) = ty {
                            self.unify_or_report(*argument_range, expected, *ty);
                        }
                    } else if let (Some(rest), Some(ty)) = (&function.variadic, ty) {
                        self.unify_or_report(*argument_range, rest.element, *ty);
                    } else {
                        // Unconsumed named formals absorb leftover positionals
                        // (R fills unmatched formals in order).
                        let next_named = named_consumed.iter().position(|consumed| !consumed);
                        match next_named {
                            Some(index) => {
                                named_consumed[index] = true;
                                if let Some(ty) = ty {
                                    let expected = function.named[index].ty;
                                    self.unify_or_report(*argument_range, expected, *ty);
                                }
                            }
                            None => {
                                self.errors.push(TypeError {
                                    range,
                                    kind: TypeErrorKind::TooManyArguments {
                                        expected: function.positional.len() + function.named.len(),
                                        found: arguments.len(),
                                    },
                                });
                                return;
                            }
                        }
                    }
                }
            }
        }
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
        assert_eq!(function.positional.len(), 1);
        assert_eq!(function.positional[0], function.ret);
    }

    #[test]
    fn call_mismatch_reports() {
        let db = RootDatabase::default();
        let check = check_source(
            &db,
            "g <- function() {\n  f <- function(x) x + 1\n  f(\"txt\")\n}",
        );
        assert!(
            check
                .errors
                .iter()
                .any(|error| matches!(error.kind, TypeErrorKind::Mismatch { .. })),
            "expected a mismatch, got {:?}",
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
