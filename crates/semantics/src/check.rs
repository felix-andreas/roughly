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
    /// `[[` on a value that supports no element extraction.
    NotAList {
        found: Ty<'db>,
    },
    /// `[` on a value the slice rules do not cover.
    UnsupportedSubset {
        found: Ty<'db>,
    },
    /// Multi-index, empty, or named-index forms (`m[i, j]`) are not modeled.
    UnsupportedIndexShape {
        index_count: usize,
    },
    /// A literal `[[` position outside a fixed-shape container (1-based).
    PositionDoesNotExist {
        position: usize,
        container: Ty<'db>,
    },
    /// A literal field name absent from a fixed-shape container.
    FieldDoesNotExist {
        field: String,
        container: Ty<'db>,
    },
    /// R rejects `$` on every atomic vector, named ones included.
    DollarOnAtomicVector {
        found: Ty<'db>,
    },
    /// `list(...)` mixing named and unnamed elements has no modeled shape.
    MixedListElements,
    /// An operator operand outside the operator's accepted family.
    InvalidOperand {
        expected: OperandExpectation,
        found: Ty<'db>,
    },
    InfiniteType,
}

/// What an operator position accepts, for error wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub enum OperandExpectation {
    Numeric,
    ScalarNumeric,
    Logical,
    Comparable,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperandShape {
    Scalar,
    Vector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComparisonFamily {
    Numeric,
    Character,
    Logical,
}

/// How an operand of an arithmetic operator classifies: a concrete numeric
/// shape, a union whose members all are (accepted member-wise), a
/// still-flexible variable or declared-numeric rigid, a vector whose element
/// is not yet concrete, an `Any`/`Unknown` short-circuit, or a hard error.
enum NumericOperand<'db> {
    Concrete(OperandShape, Atomic),
    ConcreteUnion(Vec<(OperandShape, Atomic)>),
    Flexible(Ty<'db>),
    /// A vector whose element is a generic variable/rigid (carried, to
    /// constrain) or statically untracked (`Any`/`Unknown`, carrying `None`).
    /// The shape is known — vector — even though the atomic is not.
    FlexibleVector(Option<Ty<'db>>),
    AnyUnknown,
    Invalid,
}

impl NumericOperand<'_> {
    /// The operand's member shapes: one for a concrete operand, all of them
    /// for a union.
    fn concrete_parts(&self) -> Option<Vec<(OperandShape, Atomic)>> {
        match self {
            NumericOperand::Concrete(shape, atomic) => Some(vec![(*shape, *atomic)]),
            NumericOperand::ConcreteUnion(parts) => Some(parts.clone()),
            _ => None,
        }
    }
}

/// A comparison operand whose family is not yet known: a bare variable or
/// rigid, or a vector whose element is still generic or untracked.
enum FlexibleComparisonOperand<'db> {
    Bare(Ty<'db>),
    VectorElement(Option<Ty<'db>>),
}

impl<'db> FlexibleComparisonOperand<'db> {
    fn constrainable(&self) -> Option<Ty<'db>> {
        match self {
            FlexibleComparisonOperand::Bare(ty) => Some(*ty),
            FlexibleComparisonOperand::VectorElement(ty) => *ty,
        }
    }
}

fn numeric_operand_parts<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Option<(OperandShape, Atomic)> {
    match ty.kind(db) {
        TyKind::Scalar(atomic @ (Atomic::Integer | Atomic::Double)) => {
            Some((OperandShape::Scalar, *atomic))
        }
        TyKind::Vector(element) | TyKind::NamedVector(element) => match element.kind(db) {
            TyKind::Scalar(atomic @ (Atomic::Integer | Atomic::Double)) => {
                Some((OperandShape::Vector, *atomic))
            }
            _ => None,
        },
        _ => None,
    }
}

/// A comparison operand's member shapes: one for a concrete operand, all of
/// them for a union (member-wise acceptance); `None` when any member is not
/// comparable.
fn comparison_operand_parts_list<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
) -> Option<Vec<(OperandShape, ComparisonFamily)>> {
    match ty.kind(db) {
        TyKind::Union(members) => members
            .iter()
            .map(|&member| comparison_operand_parts(db, member))
            .collect(),
        _ => comparison_operand_parts(db, ty).map(|parts| vec![parts]),
    }
}

fn comparison_operand_parts<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
) -> Option<(OperandShape, ComparisonFamily)> {
    let (shape, atomic) = match ty.kind(db) {
        TyKind::Scalar(atomic) => (OperandShape::Scalar, *atomic),
        TyKind::Vector(element) | TyKind::NamedVector(element) => match element.kind(db) {
            TyKind::Scalar(atomic) => (OperandShape::Vector, *atomic),
            _ => return None,
        },
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

fn flexible_comparison_operand<'db>(
    db: &'db dyn Db,
    ty: Ty<'db>,
) -> Option<FlexibleComparisonOperand<'db>> {
    match ty.kind(db) {
        TyKind::Var(_) | TyKind::Rigid(_) => Some(FlexibleComparisonOperand::Bare(ty)),
        TyKind::Vector(element) | TyKind::NamedVector(element) => match element.kind(db) {
            TyKind::Var(_) | TyKind::Rigid(_) => {
                Some(FlexibleComparisonOperand::VectorElement(Some(*element)))
            }
            TyKind::Any | TyKind::Unknown => Some(FlexibleComparisonOperand::VectorElement(None)),
            _ => None,
        },
        _ => None,
    }
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
                let (operator, operand) = (*operator, *operand);
                self.infer_unary(operator, operand)
            }
            ExpressionKind::Binary { operator, lhs, rhs } => {
                let (operator, lhs, rhs) = (*operator, *lhs, *rhs);
                self.infer_binary(range, operator, lhs, rhs)
            }
            ExpressionKind::Call { callee, arguments } => {
                let arguments = arguments.clone();
                self.infer_call_expression(range, *callee, &arguments)
            }
            ExpressionKind::Index {
                double,
                target,
                arguments,
            } => {
                let double = *double;
                let target = *target;
                let arguments = arguments.clone();
                self.infer_index(range, double, target, &arguments)
            }
            ExpressionKind::Field { at, target, name } => {
                let at = *at;
                let target = *target;
                let name = name.clone();
                self.infer_field(range, at, target, name)
            }
            // `pkg::name` resolves the name through the global environment
            // (which package's namespace actually exports it is not modeled).
            ExpressionKind::Namespace { name, .. } => match name
                .clone()
                .and_then(|name| self.globals.and_then(|globals| globals.scheme(&name)))
            {
                Some(namespace_scheme) => self.instantiate(&namespace_scheme),
                None => self.unknown(),
            },
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
                self.expect_scalar_logical(*condition);
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
                self.expect_scalar_logical(*condition);
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
            LiteralKind::Integer(_) => scalar(self.db, Atomic::Integer),
            LiteralKind::Double(_) => scalar(self.db, Atomic::Double),
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

    fn infer_unary(&mut self, operator: UnaryOperator, operand: ExprId) -> Ty<'db> {
        match operator {
            UnaryOperator::Minus => self.infer_unary_minus(operand),
            UnaryOperator::Not => self.infer_unary_not(operand),
            // Unary `+`, `~` formulas, and `?` help are unsupported
            // constructs: sound-by-refusal Unknown.
            UnaryOperator::Plus | UnaryOperator::Tilde | UnaryOperator::Help => {
                self.infer(operand);
                self.unknown()
            }
        }
    }

    /// Negation is elementwise and type-preserving.
    fn infer_unary_minus(&mut self, operand: ExprId) -> Ty<'db> {
        let operand_range = self.module.expression(operand).range;
        let inferred = self.infer(operand);
        let resolved = self.table.resolve(self.db, inferred);
        match self.classify_numeric_operand(resolved) {
            NumericOperand::Concrete(shape, atomic) => self.shaped(shape, atomic),
            // Member-wise over a union operand: negation preserves each
            // member's shape and atomic, so the result is the same union.
            NumericOperand::ConcreteUnion(parts) => {
                let members: Vec<Ty<'db>> = parts
                    .into_iter()
                    .map(|(shape, atomic)| self.shaped(shape, atomic))
                    .collect();
                union_of(self.db, members)
            }
            NumericOperand::Flexible(ty) => {
                self.constrain_numeric_flexible(operand_range, ty);
                ty
            }
            // A generic-element vector keeps its element (constrained
            // numeric); an untracked element stays untracked.
            NumericOperand::FlexibleVector(element) => {
                if let Some(element) = element {
                    self.constrain_numeric_flexible(operand_range, element);
                }
                resolved
            }
            NumericOperand::AnyUnknown => self.unknown(),
            NumericOperand::Invalid => {
                self.errors.push(TypeError {
                    range: operand_range,
                    kind: TypeErrorKind::InvalidOperand {
                        expected: OperandExpectation::Numeric,
                        found: resolved,
                    },
                });
                self.unknown()
            }
        }
    }

    fn infer_unary_not(&mut self, operand: ExprId) -> Ty<'db> {
        let operand_range = self.module.expression(operand).range;
        let inferred = self.infer(operand);
        let resolved = self.table.resolve(self.db, inferred);
        match resolved.kind(self.db) {
            TyKind::Scalar(Atomic::Logical) => scalar(self.db, Atomic::Logical),
            TyKind::Vector(element) | TyKind::NamedVector(element)
                if matches!(
                    element.kind(self.db),
                    TyKind::Scalar(Atomic::Logical)
                        | TyKind::Var(_)
                        | TyKind::Any
                        | TyKind::Unknown
                ) =>
            {
                Ty::new(self.db, TyKind::Vector(scalar(self.db, Atomic::Logical)))
            }
            TyKind::Any | TyKind::Unknown => self.unknown(),
            TyKind::Var(_) | TyKind::Rigid(_) => {
                self.unify_or_report(operand_range, scalar(self.db, Atomic::Logical), resolved);
                scalar(self.db, Atomic::Logical)
            }
            _ => {
                self.errors.push(TypeError {
                    range: operand_range,
                    kind: TypeErrorKind::InvalidOperand {
                        expected: OperandExpectation::Logical,
                        found: resolved,
                    },
                });
                self.unknown()
            }
        }
    }

    fn infer_binary(
        &mut self,
        range: TextRange,
        operator: BinaryOperator,
        lhs: ExprId,
        rhs: ExprId,
    ) -> Ty<'db> {
        use BinaryOperator::*;
        match operator {
            Add | Subtract | Multiply | Modulo | IntegerDivide => {
                self.infer_binary_numeric(range, lhs, rhs, false)
            }
            // `/` and `^` always produce doubles.
            Divide | Power => self.infer_binary_numeric(range, lhs, rhs, true),
            Sequence => self.infer_colon(lhs, rhs),
            Less | Greater | LessEq | GreaterEq | Equal | NotEqual => {
                self.infer_compare(range, lhs, rhs)
            }
            And2 | Or2 => {
                self.expect_scalar_logical(lhs);
                self.expect_scalar_logical(rhs);
                scalar(self.db, Atomic::Logical)
            }
            // Elementwise `&`/`|`, `%op%` specials, the `|>` pipe, `~`
            // formulas, and `?` help are unsupported constructs:
            // sound-by-refusal Unknown (the operands still infer).
            And | Or | Special | Pipe | Tilde | Help => {
                self.infer(lhs);
                self.infer(rhs);
                self.unknown()
            }
        }
    }

    /// The condition of `if`/`while` and the operands of `&&`/`||` must be
    /// scalar logicals; a still-flexible operand binds to `logical`.
    fn expect_scalar_logical(&mut self, condition: ExprId) {
        let condition_range = self.module.expression(condition).range;
        let inferred = self.infer(condition);
        let resolved = self.table.resolve(self.db, inferred);
        self.unify_or_report(condition_range, scalar(self.db, Atomic::Logical), resolved);
    }

    /// Binary arithmetic over the classified operand shapes: member-wise over
    /// unions (a vector member makes the pair a vector, both-`integer` pairs
    /// stay `integer`, any `double` — or an always-`double` operator like `/`
    /// — promotes the pair), with flexible operands collapsed onto one
    /// representative so `x + y` ties the two together.
    fn infer_binary_numeric(
        &mut self,
        range: TextRange,
        lhs: ExprId,
        rhs: ExprId,
        always_double: bool,
    ) -> Ty<'db> {
        let lhs_range = self.module.expression(lhs).range;
        let rhs_range = self.module.expression(rhs).range;
        let lhs_ty = self.infer(lhs);
        let rhs_ty = self.infer(rhs);
        let resolved_left = self.table.resolve(self.db, lhs_ty);
        let resolved_right = self.table.resolve(self.db, rhs_ty);
        let left = self.classify_numeric_operand(resolved_left);
        let right = self.classify_numeric_operand(resolved_right);

        for (operand, resolved, operand_range) in [
            (&left, resolved_left, lhs_range),
            (&right, resolved_right, rhs_range),
        ] {
            if matches!(operand, NumericOperand::Invalid) {
                self.errors.push(TypeError {
                    range: operand_range,
                    kind: TypeErrorKind::InvalidOperand {
                        expected: OperandExpectation::Numeric,
                        found: resolved,
                    },
                });
                return self.unknown();
            }
        }
        if matches!(left, NumericOperand::AnyUnknown) || matches!(right, NumericOperand::AnyUnknown)
        {
            return self.unknown();
        }

        // Collapse flexible operands (variables and numeric rigids) onto one
        // representative so `x + y` ties the two operands together; a rigid
        // pair that cannot unify is a genuine bound violation.
        let mut flexible: Option<Ty<'db>> = None;
        for operand in [&left, &right] {
            if let NumericOperand::Flexible(ty) = operand {
                match flexible {
                    Some(existing) => {
                        if let Err(error) = self.table.unify(self.db, existing, *ty) {
                            self.report_unify(range, error);
                            return self.unknown();
                        }
                    }
                    None => flexible = Some(*ty),
                }
            }
        }
        if let Some(ty) = flexible {
            self.constrain_numeric_flexible(range, ty);
        }
        // A generic vector element (`T[]`) used arithmetically must be
        // numeric; joined with the atomic-element bound it already carries,
        // the element becomes scalar-numeric.
        for operand in [&left, &right] {
            if let NumericOperand::FlexibleVector(Some(element)) = operand {
                self.constrain_numeric_flexible(range, *element);
            }
        }

        // A flexible-element vector operand fixes the result shape (vector)
        // without fixing the atomic: an always-double operation or a concrete
        // `double` (or union) partner promotes to `double[]`; an integer
        // partner promotes *into* the element, so the result keeps the
        // element; two generic elements unify; an untracked element stays
        // untracked.
        let flexible_vector_present = matches!(left, NumericOperand::FlexibleVector(_))
            || matches!(right, NumericOperand::FlexibleVector(_));
        if flexible_vector_present {
            let double_vector = Ty::new(self.db, TyKind::Vector(scalar(self.db, Atomic::Double)));
            if always_double {
                return double_vector;
            }
            let concrete_parts = left.concrete_parts().or_else(|| right.concrete_parts());
            if let Some(parts) = &concrete_parts
                && (parts.len() > 1 || parts.iter().any(|(_, atomic)| *atomic == Atomic::Double))
            {
                return double_vector;
            }
            let element = match (&left, &right) {
                (
                    NumericOperand::FlexibleVector(Some(left_element)),
                    NumericOperand::FlexibleVector(Some(right_element)),
                ) => {
                    if let Err(error) = self.table.unify(self.db, *left_element, *right_element) {
                        self.report_unify(range, error);
                        return self.unknown();
                    }
                    Some(*left_element)
                }
                (NumericOperand::FlexibleVector(None), _)
                | (_, NumericOperand::FlexibleVector(None)) => None,
                (NumericOperand::FlexibleVector(Some(element)), _)
                | (_, NumericOperand::FlexibleVector(Some(element))) => Some(*element),
                _ => None,
            };
            return Ty::new(
                self.db,
                TyKind::Vector(element.unwrap_or_else(|| self.unknown())),
            );
        }

        match (left.concrete_parts(), right.concrete_parts()) {
            // Member-wise; a single concrete operand is the one-member case,
            // so this arm also carries the ordinary concrete/concrete path.
            (Some(left_parts), Some(right_parts)) => {
                let members =
                    self.member_wise_numeric_results(&left_parts, &right_parts, always_double);
                union_of(self.db, members)
            }
            (left_parts, right_parts) => {
                let Some(flexible) = flexible else {
                    return self.unknown();
                };
                let concrete_parts = left_parts.or(right_parts);
                if let Some(parts) = &concrete_parts
                    && parts.len() > 1
                {
                    // A union operand cannot promote into a variable
                    // member-wise, so the flexible side pins to the default
                    // numeric scalar (`double`) and the operation continues
                    // member-wise.
                    self.unify_or_report(range, flexible, scalar(self.db, Atomic::Double));
                    let members = self.member_wise_numeric_results(
                        &[(OperandShape::Scalar, Atomic::Double)],
                        parts,
                        always_double,
                    );
                    return union_of(self.db, members);
                }
                let concrete = concrete_parts.and_then(|parts| parts.first().copied());
                let result_shape = match concrete {
                    Some((OperandShape::Vector, _)) => OperandShape::Vector,
                    _ => OperandShape::Scalar,
                };
                if always_double || concrete.map(|(_, atomic)| atomic) == Some(Atomic::Double) {
                    return self.shaped(result_shape, Atomic::Double);
                }
                match result_shape {
                    // `x + 1L` (and `x + y`) stay polymorphic over the
                    // numeric operand: integer promotes to whatever it
                    // resolves to, so the scalar result is the operand
                    // itself.
                    OperandShape::Scalar => flexible,
                    // A vector result cannot carry an unresolved atomic, so
                    // a flexible operand defaults to `double` here.
                    OperandShape::Vector => {
                        self.unify_or_report(range, flexible, scalar(self.db, Atomic::Double));
                        self.shaped(OperandShape::Vector, Atomic::Double)
                    }
                }
            }
        }
    }

    /// R's `:` yields an integer sequence for whole-number endpoints
    /// (`1:10` counts, via the literal rule); a `double` endpoint — or a
    /// flexible one, which may resolve to `double` — makes it `double[]`.
    /// Endpoints must be scalar numbers: the plain numeric bound would admit
    /// vectors, which R only warns about and truncates.
    fn infer_colon(&mut self, lhs: ExprId, rhs: ExprId) -> Ty<'db> {
        let mut result_atomic = Atomic::Integer;
        for operand in [lhs, rhs] {
            let operand_range = self.module.expression(operand).range;
            let whole_literal = self.is_whole_double(operand);
            let inferred = self.infer(operand);
            let resolved = self.table.resolve(self.db, inferred);
            match resolved.kind(self.db) {
                TyKind::Scalar(Atomic::Integer) => {}
                TyKind::Scalar(Atomic::Double) if whole_literal => {}
                TyKind::Scalar(Atomic::Double) => result_atomic = Atomic::Double,
                TyKind::Any | TyKind::Unknown => return self.unknown(),
                TyKind::Var(var) => {
                    if let Err(error) =
                        self.table
                            .constrain(self.db, *var, Constraint::ScalarNumeric)
                    {
                        self.report_unify(operand_range, error);
                        return self.unknown();
                    }
                    result_atomic = Atomic::Double;
                }
                TyKind::Rigid(name)
                    if matches!(
                        self.rigid_constraints.get(name),
                        Some(Constraint::ScalarNumeric)
                    ) =>
                {
                    result_atomic = Atomic::Double;
                }
                _ => {
                    self.errors.push(TypeError {
                        range: operand_range,
                        kind: TypeErrorKind::InvalidOperand {
                            expected: OperandExpectation::ScalarNumeric,
                            found: resolved,
                        },
                    });
                    return self.unknown();
                }
            }
        }
        Ty::new(self.db, TyKind::Vector(scalar(self.db, result_atomic)))
    }

    /// Comparisons: both sides must share a comparison family (numeric,
    /// character, logical — member-wise over unions), a flexible operand
    /// compared against a concrete numeric partner is constrained numeric,
    /// and the result is `logical` shaped element-wise (a vector member
    /// compares to `logical[]`).
    fn infer_compare(&mut self, range: TextRange, lhs: ExprId, rhs: ExprId) -> Ty<'db> {
        let lhs_range = self.module.expression(lhs).range;
        let rhs_range = self.module.expression(rhs).range;
        let lhs_ty = self.infer(lhs);
        let rhs_ty = self.infer(rhs);
        let resolved_left = self.table.resolve(self.db, lhs_ty);
        let resolved_right = self.table.resolve(self.db, rhs_ty);
        if matches!(resolved_left.kind(self.db), TyKind::Any | TyKind::Unknown)
            || matches!(resolved_right.kind(self.db), TyKind::Any | TyKind::Unknown)
        {
            return self.unknown();
        }

        let left_parts = comparison_operand_parts_list(self.db, resolved_left);
        let right_parts = comparison_operand_parts_list(self.db, resolved_right);
        let left_flexible = flexible_comparison_operand(self.db, resolved_left);
        let right_flexible = flexible_comparison_operand(self.db, resolved_right);

        for (parts, flexible, resolved, operand_range) in [
            (&left_parts, &left_flexible, resolved_left, lhs_range),
            (&right_parts, &right_flexible, resolved_right, rhs_range),
        ] {
            if parts.is_none() && flexible.is_none() {
                self.errors.push(TypeError {
                    range: operand_range,
                    kind: TypeErrorKind::InvalidOperand {
                        expected: OperandExpectation::Comparable,
                        found: resolved,
                    },
                });
                return self.unknown();
            }
        }

        // Two concrete operands must belong to the same comparison family,
        // member-wise: every shape the left union can take must be comparable
        // with every shape of the right.
        if let (Some(left_parts), Some(right_parts)) = (&left_parts, &right_parts)
            && left_parts.iter().any(|(_, left_family)| {
                right_parts
                    .iter()
                    .any(|(_, right_family)| left_family != right_family)
            })
        {
            self.errors.push(TypeError {
                range,
                kind: TypeErrorKind::Mismatch {
                    expected: resolved_left,
                    found: resolved_right,
                },
            });
            return self.unknown();
        }

        // A flexible operand compared against a concrete numeric operand is
        // constrained numeric; comparison against a non-numeric family leaves
        // it free (the system has no character-or-logical constraint).
        let all_numeric = |parts: &Option<Vec<(OperandShape, ComparisonFamily)>>| {
            parts.as_ref().is_some_and(|parts| {
                parts
                    .iter()
                    .all(|(_, family)| *family == ComparisonFamily::Numeric)
            })
        };
        if let Some(flexible) = &left_flexible
            && all_numeric(&right_parts)
            && let Some(ty) = flexible.constrainable()
        {
            self.constrain_numeric_flexible(lhs_range, ty);
        }
        if let Some(flexible) = &right_flexible
            && all_numeric(&left_parts)
            && let Some(ty) = flexible.constrainable()
        {
            self.constrain_numeric_flexible(rhs_range, ty);
        }

        let shapes = |parts: &Option<Vec<(OperandShape, ComparisonFamily)>>,
                      flexible: &Option<FlexibleComparisonOperand<'db>>|
         -> Vec<OperandShape> {
            match (parts, flexible) {
                (Some(parts), _) => parts.iter().map(|(shape, _)| *shape).collect(),
                (None, Some(FlexibleComparisonOperand::VectorElement(_))) => {
                    vec![OperandShape::Vector]
                }
                _ => vec![OperandShape::Scalar],
            }
        };
        let left_shapes = shapes(&left_parts, &left_flexible);
        let right_shapes = shapes(&right_parts, &right_flexible);
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
                results.push(self.shaped(result_shape, Atomic::Logical));
            }
        }
        union_of(self.db, results)
    }

    fn shaped(&self, shape: OperandShape, atomic: Atomic) -> Ty<'db> {
        let element = scalar(self.db, atomic);
        match shape {
            OperandShape::Scalar => element,
            OperandShape::Vector => Ty::new(self.db, TyKind::Vector(element)),
        }
    }

    fn member_wise_numeric_results(
        &self,
        left_parts: &[(OperandShape, Atomic)],
        right_parts: &[(OperandShape, Atomic)],
        always_double: bool,
    ) -> Vec<Ty<'db>> {
        let mut results = Vec::with_capacity(left_parts.len() * right_parts.len());
        for (left_shape, left_atomic) in left_parts {
            for (right_shape, right_atomic) in right_parts {
                let shape = if *left_shape == OperandShape::Vector
                    || *right_shape == OperandShape::Vector
                {
                    OperandShape::Vector
                } else {
                    OperandShape::Scalar
                };
                let atomic = if always_double {
                    Atomic::Double
                } else if *left_atomic == Atomic::Integer && *right_atomic == Atomic::Integer {
                    Atomic::Integer
                } else {
                    Atomic::Double
                };
                results.push(self.shaped(shape, atomic));
            }
        }
        results
    }

    /// Constrain a flexible operand (variable or rigid) to be numeric: a
    /// variable's constraint joins through the lattice; a rigid satisfies it
    /// only when its declaration promised numeric-ness.
    fn constrain_numeric_flexible(&mut self, range: TextRange, ty: Ty<'db>) {
        match ty.kind(self.db) {
            TyKind::Var(var) => {
                if let Err(error) = self.table.constrain(self.db, *var, Constraint::Numeric) {
                    self.report_unify(range, error);
                }
            }
            TyKind::Rigid(name) => {
                if !matches!(
                    self.rigid_constraints.get(name),
                    Some(Constraint::Numeric | Constraint::ScalarNumeric)
                ) {
                    self.errors.push(TypeError {
                        range,
                        kind: TypeErrorKind::ConstraintViolation {
                            constraint: Constraint::Numeric,
                            found: ty,
                        },
                    });
                }
            }
            _ => {}
        }
    }

    /// How an arithmetic operand classifies once resolved.
    fn classify_numeric_operand(&self, resolved: Ty<'db>) -> NumericOperand<'db> {
        if let Some(parts) = numeric_operand_parts(self.db, resolved) {
            return NumericOperand::Concrete(parts.0, parts.1);
        }
        match resolved.kind(self.db) {
            // A union operand is numeric when every member is; any
            // non-numeric member makes the whole operand invalid (the error
            // then shows the full union type).
            TyKind::Union(members) => {
                let mut parts = Vec::with_capacity(members.len());
                for &member in members {
                    match numeric_operand_parts(self.db, member) {
                        Some(part) => parts.push(part),
                        None => return NumericOperand::Invalid,
                    }
                }
                NumericOperand::ConcreteUnion(parts)
            }
            TyKind::Var(_) => NumericOperand::Flexible(resolved),
            TyKind::Rigid(name) => match self.rigid_constraints.get(name) {
                Some(Constraint::Numeric | Constraint::ScalarNumeric) => {
                    NumericOperand::Flexible(resolved)
                }
                _ => NumericOperand::Invalid,
            },
            TyKind::Vector(element) | TyKind::NamedVector(element) => match element.kind(self.db) {
                TyKind::Var(_) => NumericOperand::FlexibleVector(Some(*element)),
                TyKind::Rigid(name)
                    if matches!(
                        self.rigid_constraints.get(name),
                        Some(Constraint::Numeric | Constraint::ScalarNumeric)
                    ) =>
                {
                    NumericOperand::FlexibleVector(Some(*element))
                }
                TyKind::Any | TyKind::Unknown => NumericOperand::FlexibleVector(None),
                _ => NumericOperand::Invalid,
            },
            TyKind::Any | TyKind::Unknown => NumericOperand::AnyUnknown,
            _ => NumericOperand::Invalid,
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

    /// The call entry point: shape-constructing builtins (`c`, `list`,
    /// `switch`) intercept first, an overloaded stub callee resolves per call
    /// site (each candidate probed in declaration order), and everything else
    /// infers the callee and dispatches on its type.
    fn infer_call_expression(
        &mut self,
        range: TextRange,
        callee: ExprId,
        arguments: &[Argument],
    ) -> Ty<'db> {
        if let ExpressionKind::NameRef(name) = &self.module.expression(callee).kind {
            let builtin = match name.as_str() {
                "c" => Some(self.infer_combine(arguments)),
                "list" => Some(self.infer_list(range, arguments)),
                "switch" => Some(self.infer_switch(range, arguments)),
                _ => None,
            };
            if let Some(ty) = builtin {
                return ty;
            }
        }
        if let Some(ty) = self.try_overloaded_call(range, callee, arguments) {
            return ty;
        }
        let callee_ty = self.infer(callee);
        let call_arguments = self.infer_call_arguments(range, arguments);
        self.dispatch_call(range, callee_ty, &call_arguments)
    }

    /// `c(...)` follows R's atomic coercion hierarchy (logical < integer <
    /// double < complex < character; `raw` only combines with itself), drops
    /// `NULL` arguments (`c(x, NULL)` is `c(x)`, `c()` is `NULL`), and keeps
    /// names: an all-named call builds a map-like vector.
    fn infer_combine(&mut self, arguments: &[Argument]) -> Ty<'db> {
        if arguments.is_empty() {
            return crate::types::null(self.db);
        }
        let mut item_atomic: Option<Atomic> = None;
        let mut all_arguments_are_named = true;
        let mut saw_non_null_argument = false;
        let mut result_indeterminate = false;
        for argument in arguments {
            let Some(value) = argument.value else {
                continue;
            };
            let argument_range = self.module.expression(value).range;
            let inferred = self.infer(value);
            let resolved = self.table.resolve(self.db, inferred);
            if matches!(resolved.kind(self.db), TyKind::Null) {
                continue;
            }
            saw_non_null_argument = true;
            all_arguments_are_named &= argument.name.is_some();
            // A non-concrete argument whose element atomic is not statically
            // known — `Any`, `Unknown` (which must never cascade), or an
            // unresolved variable (`function(x) c(x, 1L)`) — cannot pin the
            // combined element type: the result is `Unknown` rather than a
            // rejection or an unsound concrete claim. The variable stays
            // unconstrained, mirroring `$`/`[[`/`[` on the same subject.
            match resolved.kind(self.db) {
                TyKind::Any | TyKind::Unknown | TyKind::Var(_) | TyKind::Rigid(_) => {
                    result_indeterminate = true;
                    continue;
                }
                _ => {}
            }
            // A union argument combines member-wise; its `NULL` members
            // contribute nothing (the idiomatic accumulator seeded with
            // `c()` has type `T[] | NULL` at the loop join and is no error).
            let argument_atomics: Option<Vec<Atomic>> = match resolved.kind(self.db).clone() {
                TyKind::Union(members) => members
                    .iter()
                    .filter(|member| !matches!(member.kind(self.db), TyKind::Null))
                    .map(|&member| combine_operand_atomic(self.db, member))
                    .collect(),
                _ => combine_operand_atomic(self.db, resolved).map(|atomic| vec![atomic]),
            };
            let Some(argument_atomics) = argument_atomics.filter(|atomics| !atomics.is_empty())
            else {
                self.errors.push(TypeError {
                    range: argument_range,
                    kind: TypeErrorKind::Mismatch {
                        expected: scalar(self.db, Atomic::Integer),
                        found: resolved,
                    },
                });
                result_indeterminate = true;
                continue;
            };
            for current_atomic in argument_atomics {
                item_atomic = Some(match item_atomic {
                    Some(previous_atomic) => {
                        match promote_combine_atomic(previous_atomic, current_atomic) {
                            Some(promoted) => promoted,
                            None => {
                                self.errors.push(TypeError {
                                    range: argument_range,
                                    kind: TypeErrorKind::Mismatch {
                                        expected: scalar(self.db, previous_atomic),
                                        found: resolved,
                                    },
                                });
                                result_indeterminate = true;
                                previous_atomic
                            }
                        }
                    }
                    None => current_atomic,
                });
            }
        }
        if !saw_non_null_argument {
            return crate::types::null(self.db);
        }
        if result_indeterminate {
            return self.unknown();
        }
        let element = scalar(self.db, item_atomic.unwrap_or(Atomic::Integer));
        Ty::new(
            self.db,
            if all_arguments_are_named {
                TyKind::NamedVector(element)
            } else {
                TyKind::Vector(element)
            },
        )
    }

    /// `list(...)` builds the fixed shapes: all-unnamed → tuple-like,
    /// all-named → record-like, mixed → no modeled shape.
    fn infer_list(&mut self, range: TextRange, arguments: &[Argument]) -> Ty<'db> {
        if arguments.is_empty() {
            return Ty::new(self.db, TyKind::Tuple(Vec::new()));
        }
        let all_named = arguments.iter().all(|argument| argument.name.is_some());
        let all_unnamed = arguments.iter().all(|argument| argument.name.is_none());
        if !(all_named || all_unnamed) {
            for argument in arguments {
                if let Some(value) = argument.value {
                    self.infer(value);
                }
            }
            self.errors.push(TypeError {
                range,
                kind: TypeErrorKind::MixedListElements,
            });
            return self.unknown();
        }
        if all_named {
            let mut fields = Vec::with_capacity(arguments.len());
            for argument in arguments {
                let ty = match argument.value {
                    Some(value) => {
                        let inferred = self.infer(value);
                        self.table.resolve(self.db, inferred)
                    }
                    None => self.unknown(),
                };
                let name = argument.name.clone().expect("all_named was checked");
                fields.push(crate::types::RecordField {
                    name: Name::new(self.db, name),
                    ty,
                    optional: false,
                });
            }
            Ty::new(self.db, TyKind::Record(fields))
        } else {
            let mut items = Vec::with_capacity(arguments.len());
            for argument in arguments {
                let ty = match argument.value {
                    Some(value) => {
                        let inferred = self.infer(value);
                        self.table.resolve(self.db, inferred)
                    }
                    None => self.unknown(),
                };
                items.push(ty);
            }
            Ty::new(self.db, TyKind::Tuple(items))
        }
    }

    /// `switch(subject, a = ..., b = ..., default)` selects one branch by the
    /// subject's runtime value. Selection cannot be modeled statically, but
    /// every branch IS checked, and the call's type is the union of the
    /// branch values. R returns invisible `NULL` when nothing matches, so
    /// `NULL` joins the union unless a default (unnamed) branch exists; a
    /// named branch with no value falls through and contributes no type.
    fn infer_switch(&mut self, range: TextRange, arguments: &[Argument]) -> Ty<'db> {
        let Some((subject, branches)) = arguments.split_first() else {
            self.errors.push(TypeError {
                range,
                kind: TypeErrorKind::ArityMismatch {
                    expected: 1,
                    found: 0,
                },
            });
            return self.unknown();
        };
        if let Some(value) = subject.value {
            self.infer(value);
        }
        let mut members = Vec::with_capacity(branches.len() + 1);
        let mut has_default = false;
        for branch in branches {
            if branch.name.is_none() {
                has_default = true;
            }
            let Some(value) = branch.value else {
                continue;
            };
            let ty = self.infer(value);
            members.push(self.table.resolve(self.db, ty));
        }
        if !has_default {
            members.push(crate::types::null(self.db));
        }
        union_of(self.db, members)
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
        let name = match &self.module.expression(callee).kind {
            ExpressionKind::NameRef(name) => {
                if self.naming.resolutions.contains_key(&callee) {
                    return None;
                }
                name.clone()
            }
            // Namespace access cannot be shadowed by a local binding.
            ExpressionKind::Namespace {
                name: Some(name), ..
            } => name.clone(),
            _ => return None,
        };
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
        match &self.module.expression(value).kind {
            ExpressionKind::Literal(LiteralKind::Double(text)) => {
                crate::hir::is_whole_number_double(text)
            }
            _ => false,
        }
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

    /// `x[[i]]` / `x[i]`: the subject and every index infer first regardless
    /// of shape, so names inside an unsupported form (`m[i, j]`) still
    /// resolve and get their own diagnostics.
    fn infer_index(
        &mut self,
        range: TextRange,
        double: bool,
        target: ExprId,
        arguments: &[Argument],
    ) -> Ty<'db> {
        let target_ty = self.infer(target);
        for argument in arguments {
            if let Some(value) = argument.value {
                self.infer(value);
            }
        }
        let subject = self.table.resolve(self.db, target_ty);
        // An Unknown/Any subject stays Unknown/Any even under an unsupported
        // index shape — the subject's own gap was already diagnosed, so
        // `m[i, j]` must not cascade an arity error. A sealed nominal
        // supports value-dependent indexing of any shape at runtime
        // (`df[rows, cols]`), none of it modeled — Unknown before the
        // index-arity check, so idiomatic two-index subsetting is no error.
        match subject.kind(self.db) {
            TyKind::Unknown => return self.unknown(),
            TyKind::Any => return crate::types::any(self.db),
            TyKind::Named(..) => return self.unknown(),
            _ => {}
        }
        if arguments.len() != 1 || arguments[0].name.is_some() {
            self.errors.push(TypeError {
                range,
                kind: TypeErrorKind::UnsupportedIndexShape {
                    index_count: arguments.len(),
                },
            });
            return self.unknown();
        }
        let result = if double {
            let index = arguments[0]
                .value
                .map(|value| self.module.expression(value).kind.clone());
            self.extract_result(range, subject, index.as_ref())
        } else {
            self.subset_result(range, subject)
        };
        match result {
            Ok(ty) => ty,
            Err(error) => {
                self.errors.push(error);
                self.unknown()
            }
        }
    }

    /// `[[` — single-element extraction.
    fn extract_result(
        &mut self,
        range: TextRange,
        subject: Ty<'db>,
        index: Option<&ExpressionKind>,
    ) -> Result<Ty<'db>, TypeError<'db>> {
        // Member-wise over a union subject: `[[` must be valid on every shape
        // the subject can take, and the element's type is the join of the
        // per-member results; a failing member reports the full union.
        if let TyKind::Union(members) = subject.kind(self.db).clone() {
            let mut results = Vec::with_capacity(members.len());
            for member in members {
                let result = self
                    .extract_result(range, member, index)
                    .map_err(|error| widen_error_container(error, subject))?;
                results.push(result);
            }
            return Ok(union_of(self.db, results));
        }
        let literal_position = index.and_then(crate::hir::integer_literal_position);
        let literal_name = match index {
            Some(ExpressionKind::Literal(LiteralKind::String(name))) => Some(name.clone()),
            _ => None,
        };
        match subject.kind(self.db).clone() {
            TyKind::Unknown => Ok(self.unknown()),
            TyKind::Any => Ok(crate::types::any(self.db)),
            TyKind::Scalar(atomic) => Ok(scalar(self.db, atomic)),
            TyKind::Vector(element) | TyKind::List(element) => Ok(element),
            // A literal name may be absent at runtime (`T | NULL`), while
            // positional and computed access extract an element like R does
            // on any vector or list.
            TyKind::NamedVector(element) | TyKind::NamedList(element) => {
                Ok(if literal_name.is_some() {
                    union_of(self.db, [element, crate::types::null(self.db)])
                } else {
                    element
                })
            }
            // A computed position could reach any item, so the extraction is
            // the union of the item types; only a *literal* position is
            // precise (and still errors when out of range).
            TyKind::Tuple(items) => match literal_position {
                Some(position) => match items.get(position) {
                    Some(&item) => Ok(item),
                    None => Err(TypeError {
                        range,
                        kind: TypeErrorKind::PositionDoesNotExist {
                            position: position + 1,
                            container: subject,
                        },
                    }),
                },
                None => Ok(union_of(self.db, items)),
            },
            TyKind::Record(fields) => {
                // Record fields are declaration-ordered, so a positional `[[`
                // extracts the field at that position exactly like a tuple
                // item.
                if let Some(position) = literal_position {
                    return match fields.get(position) {
                        Some(field) => Ok(field.ty),
                        None => Err(TypeError {
                            range,
                            kind: TypeErrorKind::PositionDoesNotExist {
                                position: position + 1,
                                container: subject,
                            },
                        }),
                    };
                }
                match literal_name {
                    // A computed name could reach any field — the
                    // dispatch-table idiom, `handlers[[name]](...)`.
                    None => Ok(union_of(self.db, fields.iter().map(|field| field.ty))),
                    Some(name) => {
                        match fields.iter().find(|field| field.name.text(self.db) == name) {
                            Some(field) => Ok(field.ty),
                            None => Err(TypeError {
                                range,
                                kind: TypeErrorKind::FieldDoesNotExist {
                                    field: name,
                                    container: subject,
                                },
                            }),
                        }
                    }
                }
            }
            // A sealed nominal and an unresolved inference variable both
            // support element access the system cannot model — sound-by-
            // refusal Unknown, never a rejection (idiomatic R walks generic
            // data this way: `function(x) x[[1L]]`). The variable stays
            // unconstrained.
            TyKind::Named(..) | TyKind::Var(_) | TyKind::Rigid(_) => Ok(self.unknown()),
            _ => Err(TypeError {
                range,
                kind: TypeErrorKind::NotAList { found: subject },
            }),
        }
    }

    /// `[` — the list slice.
    fn subset_result(
        &mut self,
        range: TextRange,
        subject: Ty<'db>,
    ) -> Result<Ty<'db>, TypeError<'db>> {
        // Member-wise over a union subject, like `[[`.
        if let TyKind::Union(members) = subject.kind(self.db).clone() {
            let mut results = Vec::with_capacity(members.len());
            for member in members {
                let result = self
                    .subset_result(range, member)
                    .map_err(|error| widen_error_container(error, subject))?;
                results.push(result);
            }
            return Ok(union_of(self.db, results));
        }
        match subject.kind(self.db).clone() {
            TyKind::Unknown => Ok(self.unknown()),
            TyKind::Any => Ok(crate::types::any(self.db)),
            TyKind::List(_) | TyKind::NamedList(_) => Ok(subject),
            // A `[` slice of a fixed-shape list is a sub-list that can
            // contain any of the item types, so the element type is their
            // union (collapsing back for a homogeneous list; slicing the
            // empty list yields `list[NULL]`).
            TyKind::Tuple(items) => Ok(Ty::new(self.db, TyKind::List(union_of(self.db, items)))),
            TyKind::Record(fields) => Ok(Ty::new(
                self.db,
                TyKind::NamedList(union_of(self.db, fields.iter().map(|field| field.ty))),
            )),
            // Sound-by-refusal, as for `[[`.
            TyKind::Named(..) | TyKind::Var(_) | TyKind::Rigid(_) => Ok(self.unknown()),
            _ => Err(TypeError {
                range,
                kind: TypeErrorKind::UnsupportedSubset { found: subject },
            }),
        }
    }

    /// `x$name` behaves as `[["name"]]` on lists and records — but not on
    /// atomic vectors, which R rejects outright. `x@name` (S4 slot access) is
    /// not modeled: sound-by-refusal Unknown.
    fn infer_field(
        &mut self,
        range: TextRange,
        at: bool,
        target: ExprId,
        name: Option<String>,
    ) -> Ty<'db> {
        let target_ty = self.infer(target);
        if at {
            return self.unknown();
        }
        let Some(name) = name else {
            return self.unknown();
        };
        let subject = self.table.resolve(self.db, target_ty);
        match self.dollar_result(range, subject, &name) {
            Ok(ty) => ty,
            Err(error) => {
                self.errors.push(error);
                self.unknown()
            }
        }
    }

    fn dollar_result(
        &mut self,
        range: TextRange,
        subject: Ty<'db>,
        name: &str,
    ) -> Result<Ty<'db>, TypeError<'db>> {
        // Member-wise over a union subject: the field must exist on every
        // shape the subject can take.
        if let TyKind::Union(members) = subject.kind(self.db).clone() {
            let mut results = Vec::with_capacity(members.len());
            for member in members {
                let result = self
                    .dollar_result(range, member, name)
                    .map_err(|error| widen_error_container(error, subject))?;
                results.push(result);
            }
            return Ok(union_of(self.db, results));
        }
        match subject.kind(self.db).clone() {
            TyKind::Unknown => Ok(self.unknown()),
            TyKind::Any => Ok(crate::types::any(self.db)),
            // R rejects `$` on every atomic vector ("$ operator is invalid
            // for atomic vectors"), named ones included — element extraction
            // is `[[`'s job.
            TyKind::Scalar(_) | TyKind::Vector(_) | TyKind::NamedVector(_) => Err(TypeError {
                range,
                kind: TypeErrorKind::DollarOnAtomicVector { found: subject },
            }),
            TyKind::NamedList(element) => {
                Ok(union_of(self.db, [element, crate::types::null(self.db)]))
            }
            TyKind::Record(fields) => {
                match fields.iter().find(|field| field.name.text(self.db) == name) {
                    Some(field) => Ok(field.ty),
                    None => Err(TypeError {
                        range,
                        kind: TypeErrorKind::FieldDoesNotExist {
                            field: name.to_owned(),
                            container: subject,
                        },
                    }),
                }
            }
            TyKind::Tuple(_) | TyKind::List(_) => Err(TypeError {
                range,
                kind: TypeErrorKind::FieldDoesNotExist {
                    field: name.to_owned(),
                    container: subject,
                },
            }),
            // Sound-by-refusal for sealed nominals (`df$col` is the most
            // idiomatic R there is) and unresolved variables
            // (`function(node) node$value`).
            TyKind::Named(..) | TyKind::Var(_) | TyKind::Rigid(_) => Ok(self.unknown()),
            _ => Err(TypeError {
                range,
                kind: TypeErrorKind::NotAList { found: subject },
            }),
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

/// The statically known element atomic of a `c(...)` operand: a scalar or a
/// vector with a concrete scalar element; anything else cannot combine.
fn combine_operand_atomic<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Option<Atomic> {
    match ty.kind(db) {
        TyKind::Scalar(atomic) => Some(*atomic),
        TyKind::Vector(element) | TyKind::NamedVector(element) => match element.kind(db) {
            TyKind::Scalar(atomic) => Some(*atomic),
            _ => None,
        },
        _ => None,
    }
}

/// R's atomic coercion hierarchy for `c(...)`: logical < integer < double <
/// complex < character (`c(1L, NA)` is `integer`, `c(1L, "a")` is
/// `character`); `raw` does not participate and only combines with itself.
fn promote_combine_atomic(left: Atomic, right: Atomic) -> Option<Atomic> {
    if left == right {
        return Some(left);
    }
    let rank = |atomic: Atomic| match atomic {
        Atomic::Logical => Some(0u8),
        Atomic::Integer => Some(1),
        Atomic::Double => Some(2),
        Atomic::Complex => Some(3),
        Atomic::Character => Some(4),
        Atomic::Raw => None,
    };
    Some(match rank(left)?.max(rank(right)?) {
        0 => Atomic::Logical,
        1 => Atomic::Integer,
        2 => Atomic::Double,
        3 => Atomic::Complex,
        _ => Atomic::Character,
    })
}

/// A failing union member reports the full union — the subject's actual type —
/// not the single member that failed.
fn widen_error_container<'db>(mut error: TypeError<'db>, union: Ty<'db>) -> TypeError<'db> {
    match &mut error.kind {
        TypeErrorKind::NotAList { found }
        | TypeErrorKind::UnsupportedSubset { found }
        | TypeErrorKind::DollarOnAtomicVector { found } => *found = union,
        TypeErrorKind::PositionDoesNotExist { container, .. }
        | TypeErrorKind::FieldDoesNotExist { container, .. } => *container = union,
        _ => {}
    }
    error
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
    fn arithmetic_ties_two_flexible_operands() {
        let db = RootDatabase::default();
        // `<T: numeric> fn(a: T, b: T) -> T`: one binder shared by all three.
        let check = check_source(&db, "add <- function(a, b) a + b");
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        let scheme = check.scheme.expect("scheme");
        assert_eq!(scheme.binders.len(), 1, "{scheme:?}");
        let TyKind::Function(function) = scheme.body.kind(&db) else {
            panic!()
        };
        assert_eq!(function.named[0].ty, function.ret);
        assert_eq!(function.named[1].ty, function.ret);
    }

    #[test]
    fn division_is_always_double_and_keeps_the_operand_generic() {
        let db = RootDatabase::default();
        let check = check_source(&db, "half <- function(x) x / 2");
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        let scheme = check.scheme.clone().expect("scheme");
        assert_eq!(scheme.binders.len(), 1, "x stays generic: {scheme:?}");
        assert_eq!(scheme_ret(&db, &check), scalar(&db, Atomic::Double));
    }

    #[test]
    fn arithmetic_shape_rules_over_vectors() {
        let db = RootDatabase::default();
        // integer[] + integer stays integer[].
        let vector = check_source(&db, "f <- function() c(1L, 2L) + 1L");
        assert!(vector.errors.is_empty(), "{:?}", vector.errors);
        assert!(matches!(
            scheme_ret(&db, &vector).kind(&db),
            TyKind::Vector(element) if *element == scalar(&db, Atomic::Integer)
        ));
        // integer[] / integer promotes to double[].
        let divided = check_source(&db, "f <- function() c(1L, 2L) / 2L");
        assert!(divided.errors.is_empty(), "{:?}", divided.errors);
        assert!(matches!(
            scheme_ret(&db, &divided).kind(&db),
            TyKind::Vector(element) if *element == scalar(&db, Atomic::Double)
        ));
        // `%%` is arithmetic, not an opaque special.
        let modulo = check_source(&db, "f <- function() 7L %% 2L");
        assert!(modulo.errors.is_empty(), "{:?}", modulo.errors);
        assert_eq!(scheme_ret(&db, &modulo), scalar(&db, Atomic::Integer));
        // A non-numeric operand reports the operand, not the whole range.
        let bad = check_source(&db, "f <- function() \"a\" + 1L");
        assert!(
            bad.errors.iter().any(|error| matches!(
                error.kind,
                TypeErrorKind::InvalidOperand {
                    expected: OperandExpectation::Numeric,
                    ..
                }
            )),
            "{:?}",
            bad.errors
        );
    }

    #[test]
    fn colon_yields_integer_sequences_with_double_fallback() {
        let db = RootDatabase::default();
        let literal = check_source(&db, "f <- function() 1:10");
        assert!(literal.errors.is_empty(), "{:?}", literal.errors);
        assert!(matches!(
            scheme_ret(&db, &literal).kind(&db),
            TyKind::Vector(element) if *element == scalar(&db, Atomic::Integer)
        ));
        // A flexible endpoint may resolve to double, so the claim is double[].
        let flexible = check_source(&db, "f <- function(n) 1:n");
        assert!(flexible.errors.is_empty(), "{:?}", flexible.errors);
        assert!(matches!(
            scheme_ret(&db, &flexible).kind(&db),
            TyKind::Vector(element) if *element == scalar(&db, Atomic::Double)
        ));
    }

    #[test]
    fn comparisons_share_a_family_and_shape_elementwise() {
        let db = RootDatabase::default();
        // A flexible operand against a numeric partner is constrained.
        let flexible = check_source(&db, "positive <- function(x) x > 0L");
        assert!(flexible.errors.is_empty(), "{:?}", flexible.errors);
        let scheme = flexible.scheme.clone().expect("scheme");
        assert_eq!(scheme.binders.len(), 1);
        assert_eq!(scheme.binders[0].1, Constraint::Numeric);
        assert_eq!(scheme_ret(&db, &flexible), scalar(&db, Atomic::Logical));
        // A vector member compares element-wise.
        let vector = check_source(&db, "f <- function() c(1L, 2L) > 1L");
        assert!(vector.errors.is_empty(), "{:?}", vector.errors);
        assert!(matches!(
            scheme_ret(&db, &vector).kind(&db),
            TyKind::Vector(element) if *element == scalar(&db, Atomic::Logical)
        ));
        // Cross-family comparison is a mismatch.
        let mixed = check_source(&db, "f <- function() 1L > \"a\"");
        assert!(
            mixed
                .errors
                .iter()
                .any(|error| matches!(error.kind, TypeErrorKind::Mismatch { .. })),
            "{:?}",
            mixed.errors
        );
    }

    #[test]
    fn conditions_and_short_circuit_operators_expect_scalar_logical() {
        let db = RootDatabase::default();
        let bad_condition = check_source(&db, "f <- function() if (1L) 2L");
        assert!(
            bad_condition
                .errors
                .iter()
                .any(|error| matches!(error.kind, TypeErrorKind::Mismatch { .. })),
            "{:?}",
            bad_condition.errors
        );
        let bad_and = check_source(&db, "f <- function() 1L && TRUE");
        assert!(
            bad_and
                .errors
                .iter()
                .any(|error| matches!(error.kind, TypeErrorKind::Mismatch { .. })),
            "{:?}",
            bad_and.errors
        );
        let ok = check_source(&db, "f <- function(a, b) a && b");
        assert!(ok.errors.is_empty(), "{:?}", ok.errors);
    }

    #[test]
    fn unary_operators_preserve_shape() {
        let db = RootDatabase::default();
        let negated = check_source(&db, "f <- function() -c(1L, 2L)");
        assert!(negated.errors.is_empty(), "{:?}", negated.errors);
        assert!(matches!(
            scheme_ret(&db, &negated).kind(&db),
            TyKind::Vector(element) if *element == scalar(&db, Atomic::Integer)
        ));
        let notted = check_source(&db, "f <- function() !c(TRUE, FALSE)");
        assert!(notted.errors.is_empty(), "{:?}", notted.errors);
        assert!(matches!(
            scheme_ret(&db, &notted).kind(&db),
            TyKind::Vector(element) if *element == scalar(&db, Atomic::Logical)
        ));
        let bad = check_source(&db, "f <- function() -\"a\"");
        assert!(
            bad.errors.iter().any(|error| matches!(
                error.kind,
                TypeErrorKind::InvalidOperand {
                    expected: OperandExpectation::Numeric,
                    ..
                }
            )),
            "{:?}",
            bad.errors
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

    fn scheme_ret<'db>(db: &'db RootDatabase, check: &ItemCheck<'db>) -> Ty<'db> {
        let scheme = check.scheme.clone().expect("scheme");
        let TyKind::Function(function) = scheme.body.kind(db).clone() else {
            panic!("expected a function scheme");
        };
        function.ret
    }

    #[test]
    fn list_builds_fixed_shapes_and_extraction_is_precise() {
        let db = RootDatabase::default();
        // Literal position on a tuple-like list.
        let tuple = check_source(
            &db,
            "f <- function() {\n  x <- list(1L, \"a\")\n  x[[1L]]\n}",
        );
        assert!(tuple.errors.is_empty(), "{:?}", tuple.errors);
        assert_eq!(scheme_ret(&db, &tuple), scalar(&db, Atomic::Integer));
        // Literal field via `$` on a record-like list.
        let record = check_source(
            &db,
            "f <- function() {\n  p <- list(a = 1L, b = \"s\")\n  p$b\n}",
        );
        assert!(record.errors.is_empty(), "{:?}", record.errors);
        assert_eq!(scheme_ret(&db, &record), scalar(&db, Atomic::Character));
        // A computed index is the union of the item types.
        let computed = check_source(
            &db,
            "f <- function(i) {\n  x <- list(1L, \"a\")\n  x[[i]]\n}",
        );
        assert!(computed.errors.is_empty(), "{:?}", computed.errors);
        assert!(matches!(
            scheme_ret(&db, &computed).kind(&db),
            TyKind::Union(members) if members.len() == 2
        ));
        // `[` slices into a sub-list of the union.
        let slice = check_source(&db, "f <- function() {\n  x <- list(1L, \"a\")\n  x[1L]\n}");
        assert!(slice.errors.is_empty(), "{:?}", slice.errors);
        assert!(matches!(
            scheme_ret(&db, &slice).kind(&db),
            TyKind::List(element) if matches!(element.kind(&db), TyKind::Union(_))
        ));
    }

    #[test]
    fn index_errors_report_precisely() {
        let db = RootDatabase::default();
        let out_of_range = check_source(
            &db,
            "f <- function() {\n  x <- list(1L, \"a\")\n  x[[3L]]\n}",
        );
        assert!(
            out_of_range.errors.iter().any(|error| matches!(
                error.kind,
                TypeErrorKind::PositionDoesNotExist { position: 3, .. }
            )),
            "{:?}",
            out_of_range.errors
        );
        let missing_field =
            check_source(&db, "f <- function() {\n  p <- list(a = 1L)\n  p$oops\n}");
        assert!(
            missing_field.errors.iter().any(|error| matches!(
                &error.kind,
                TypeErrorKind::FieldDoesNotExist { field, .. } if field == "oops"
            )),
            "{:?}",
            missing_field.errors
        );
        // R rejects `$` on atomic vectors, named ones included.
        let dollar_atomic = check_source(&db, "f <- function() {\n  v <- c(a = 1L)\n  v$a\n}");
        assert!(
            dollar_atomic
                .errors
                .iter()
                .any(|error| matches!(error.kind, TypeErrorKind::DollarOnAtomicVector { .. })),
            "{:?}",
            dollar_atomic.errors
        );
        // Multi-index forms are not modeled.
        let matrix = check_source(&db, "f <- function() {\n  x <- list(1L)\n  x[1L, 2L]\n}");
        assert!(
            matrix.errors.iter().any(|error| matches!(
                error.kind,
                TypeErrorKind::UnsupportedIndexShape { index_count: 2 }
            )),
            "{:?}",
            matrix.errors
        );
    }

    #[test]
    fn named_vector_extraction_is_nullable_by_name() {
        let db = RootDatabase::default();
        let check = check_source(&db, "f <- function() {\n  v <- c(a = 1L)\n  v[[\"a\"]]\n}");
        assert!(check.errors.is_empty(), "{:?}", check.errors);
        let TyKind::Union(members) = scheme_ret(&db, &check).kind(&db) else {
            panic!("expected integer | NULL");
        };
        assert!(members.contains(&scalar(&db, Atomic::Integer)));
        assert!(members.contains(&crate::types::null(&db)));
    }

    #[test]
    fn combine_promotes_drops_null_and_keeps_names() {
        let db = RootDatabase::default();
        let promoted = check_source(&db, "f <- function() c(1L, 2.5)");
        assert!(promoted.errors.is_empty());
        assert!(matches!(
            scheme_ret(&db, &promoted).kind(&db),
            TyKind::Vector(element) if *element == scalar(&db, Atomic::Double)
        ));
        let character = check_source(&db, "f <- function() c(1L, \"a\")");
        assert!(character.errors.is_empty());
        assert!(matches!(
            scheme_ret(&db, &character).kind(&db),
            TyKind::Vector(element) if *element == scalar(&db, Atomic::Character)
        ));
        let named = check_source(&db, "f <- function() c(a = 1L, b = 2L)");
        assert!(named.errors.is_empty());
        assert!(matches!(
            scheme_ret(&db, &named).kind(&db),
            TyKind::NamedVector(_)
        ));
        let null_dropped = check_source(&db, "f <- function() c(1L, NULL)");
        assert!(null_dropped.errors.is_empty());
        assert!(matches!(
            scheme_ret(&db, &null_dropped).kind(&db),
            TyKind::Vector(_)
        ));
        let empty = check_source(&db, "f <- function() c()");
        assert!(matches!(scheme_ret(&db, &empty).kind(&db), TyKind::Null));
    }

    #[test]
    fn switch_unions_branches_with_null_unless_defaulted() {
        let db = RootDatabase::default();
        let no_default = check_source(&db, "f <- function(x) switch(x, a = 1L, b = \"s\")");
        assert!(no_default.errors.is_empty(), "{:?}", no_default.errors);
        let TyKind::Union(members) = scheme_ret(&db, &no_default).kind(&db) else {
            panic!("expected a union");
        };
        assert_eq!(members.len(), 3, "integer | character | NULL: {members:?}");
        let with_default = check_source(&db, "f <- function(x) switch(x, a = 1L, 2L)");
        assert!(with_default.errors.is_empty());
        assert_eq!(
            scheme_ret(&db, &with_default),
            scalar(&db, Atomic::Integer),
            "both branches integer, no NULL member"
        );
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
