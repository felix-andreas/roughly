//! The interned type representation.
//!
//! Types are salsa-interned: a `Ty` is a copyable id, equality is id
//! comparison, and no deep clones exist anywhere in inference — the structural
//! churn that capped the legacy checker is designed out from the start.
//! Inference variables are ordinary interned types (`TyKind::Var`), so there is
//! exactly one type representation end to end.
//!
//! Unions are normalized in exactly one place, `union_of`: flatten, structural
//! dedupe keeping first occurrence, `NULL` ordered last, `Any` then `Unknown`
//! absorb the union, a singleton unwraps, and empty collapses to `NULL`. Never
//! construct `TyKind::Union` directly.

use crate::Db;

/// An interned identifier (variable names, parameter names, type names).
#[salsa::interned(debug)]
pub struct Name<'db> {
    #[returns(deref)]
    pub text: String,
}

/// An interned type: a cheap copyable id; equality is id equality.
#[salsa::interned(debug)]
pub struct Ty<'db> {
    #[returns(ref)]
    pub kind: TyKind<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub enum TyKind<'db> {
    /// The sanctioned escape hatch: compatible with everything, silently.
    Any,
    /// An absent or unmodeled fact; strict mode reports its origins.
    Unknown,
    Null,
    Scalar(Atomic),
    /// `T[]` — an atomic vector with element type `T`.
    Vector(Ty<'db>),
    /// `T[named]`.
    NamedVector(Ty<'db>),
    /// `list[T]`.
    List(Ty<'db>),
    /// `list[named: T]`.
    NamedList(Ty<'db>),
    /// `list{A, B}`.
    Tuple(Vec<Ty<'db>>),
    /// `list{a: A, b: B}` — fields keep declaration order.
    Record(Vec<RecordField<'db>>),
    Function(FunctionType<'db>),
    /// A normalized union; built only through `union_of`.
    Union(Vec<Ty<'db>>),
    /// A named (nominal or alias-expanded) type with its arguments.
    Named(Name<'db>, Vec<Ty<'db>>),
    /// A unification variable (inference-scoped; resolved through the
    /// inference table, never stored in exported schemes).
    Var(InferenceVar),
    /// A rigid (skolem) variable from an explicit `<T>` binder: refuses to
    /// bind while its scope is checked, generalizes back out afterwards.
    Rigid(Name<'db>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub enum Atomic {
    Logical,
    Integer,
    Double,
    Complex,
    Character,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, salsa::SalsaValue)]
pub struct InferenceVar(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub struct RecordField<'db> {
    pub name: Name<'db>,
    pub ty: Ty<'db>,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub struct FunctionType<'db> {
    /// Positional parameters, in order.
    pub positional: Vec<Ty<'db>>,
    /// Named parameters (declaration order; `optional` marks `[name]:`).
    pub named: Vec<RecordField<'db>>,
    /// The `...` rest parameter's element type, with the count of named
    /// parameters written before it (parameters after `...` match by name
    /// only).
    pub variadic: Option<RestParameter<'db>>,
    pub ret: Ty<'db>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub struct RestParameter<'db> {
    pub element: Ty<'db>,
    pub preceding_named: usize,
}

/// The constraint lattice on inference variables and binders. `Numeric` and
/// `AtomicElement` join to `ScalarNumeric` (a scalar integer/double).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, salsa::SalsaValue)]
pub enum Constraint {
    #[default]
    Unconstrained,
    Numeric,
    AtomicElement,
    ScalarNumeric,
}

impl Constraint {
    pub fn join(self, other: Constraint) -> Constraint {
        use Constraint::*;
        match (self, other) {
            (Unconstrained, c) | (c, Unconstrained) => c,
            (a, b) if a == b => a,
            (ScalarNumeric, _) | (_, ScalarNumeric) => ScalarNumeric,
            (Numeric, AtomicElement) | (AtomicElement, Numeric) => ScalarNumeric,
            _ => unreachable!("all constraint pairs are covered"),
        }
    }
}

/// A generalized type: binders with their constraints over a body.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub struct TypeScheme<'db> {
    pub binders: Vec<(Name<'db>, Constraint)>,
    pub body: Ty<'db>,
}

impl<'db> TypeScheme<'db> {
    pub fn monomorphic(body: Ty<'db>) -> TypeScheme<'db> {
        TypeScheme {
            binders: Vec::new(),
            body,
        }
    }
}

pub fn any(db: &dyn Db) -> Ty<'_> {
    Ty::new(db, TyKind::Any)
}

pub fn unknown(db: &dyn Db) -> Ty<'_> {
    Ty::new(db, TyKind::Unknown)
}

pub fn null(db: &dyn Db) -> Ty<'_> {
    Ty::new(db, TyKind::Null)
}

pub fn scalar(db: &dyn Db, atomic: Atomic) -> Ty<'_> {
    Ty::new(db, TyKind::Scalar(atomic))
}

/// THE union constructor: every builder, resolver, importer, and instantiation
/// path goes through here, because members can collapse after substitution.
pub fn union_of<'db>(db: &'db dyn Db, members: impl IntoIterator<Item = Ty<'db>>) -> Ty<'db> {
    let mut flat: Vec<Ty<'db>> = Vec::new();
    let mut saw_null = false;
    let mut stack: Vec<Ty<'db>> = members.into_iter().collect();
    stack.reverse();
    while let Some(member) = stack.pop() {
        match member.kind(db) {
            TyKind::Union(inner) => {
                for &inner_member in inner.iter().rev() {
                    stack.push(inner_member);
                }
            }
            TyKind::Any => return any(db),
            TyKind::Unknown => return unknown(db),
            TyKind::Null => saw_null = true,
            _ => {
                if !flat.contains(&member) {
                    flat.push(member);
                }
            }
        }
    }
    if saw_null {
        flat.push(null(db));
    }
    match flat.len() {
        0 => null(db),
        1 => flat[0],
        _ => Ty::new(db, TyKind::Union(flat)),
    }
}

/// Replace every inference variable in an (already-resolved) type with
/// `Unknown`, for values that must survive a later table rollback: a stored
/// variable id would dangle once the rollback reclaims (and later reuses) the
/// id.
pub fn erase_vars<'db>(db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
    match ty.kind(db).clone() {
        TyKind::Var(_) => unknown(db),
        TyKind::Vector(inner) => Ty::new(db, TyKind::Vector(erase_vars(db, inner))),
        TyKind::NamedVector(inner) => Ty::new(db, TyKind::NamedVector(erase_vars(db, inner))),
        TyKind::List(inner) => Ty::new(db, TyKind::List(erase_vars(db, inner))),
        TyKind::NamedList(inner) => Ty::new(db, TyKind::NamedList(erase_vars(db, inner))),
        TyKind::Tuple(items) => Ty::new(
            db,
            TyKind::Tuple(items.iter().map(|&item| erase_vars(db, item)).collect()),
        ),
        TyKind::Record(fields) => Ty::new(
            db,
            TyKind::Record(
                fields
                    .iter()
                    .map(|field| {
                        let mut field = field.clone();
                        field.ty = erase_vars(db, field.ty);
                        field
                    })
                    .collect(),
            ),
        ),
        TyKind::Function(function) => Ty::new(
            db,
            TyKind::Function(FunctionType {
                positional: function
                    .positional
                    .iter()
                    .map(|&ty| erase_vars(db, ty))
                    .collect(),
                named: function
                    .named
                    .iter()
                    .map(|field| {
                        let mut field = field.clone();
                        field.ty = erase_vars(db, field.ty);
                        field
                    })
                    .collect(),
                variadic: function.variadic.as_ref().map(|rest| {
                    let mut rest = rest.clone();
                    rest.element = erase_vars(db, rest.element);
                    rest
                }),
                ret: erase_vars(db, function.ret),
            }),
        ),
        TyKind::Union(members) => {
            union_of(db, members.iter().map(|&member| erase_vars(db, member)))
        }
        TyKind::Named(name, arguments) => Ty::new(
            db,
            TyKind::Named(
                name,
                arguments
                    .iter()
                    .map(|&argument| erase_vars(db, argument))
                    .collect(),
            ),
        ),
        _ => ty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RootDatabase;

    #[test]
    fn interning_gives_id_equality() {
        let db = RootDatabase::default();
        let a = scalar(&db, Atomic::Integer);
        let b = scalar(&db, Atomic::Integer);
        assert_eq!(a, b);
        let f1 = Ty::new(
            &db,
            TyKind::Function(FunctionType {
                positional: vec![a],
                named: Vec::new(),
                variadic: None,
                ret: scalar(&db, Atomic::Double),
            }),
        );
        let f2 = Ty::new(
            &db,
            TyKind::Function(FunctionType {
                positional: vec![b],
                named: Vec::new(),
                variadic: None,
                ret: scalar(&db, Atomic::Double),
            }),
        );
        assert_eq!(f1, f2);
    }

    #[test]
    fn union_normalization() {
        let db = RootDatabase::default();
        let int = scalar(&db, Atomic::Integer);
        let chr = scalar(&db, Atomic::Character);

        // Flatten + dedupe keeping first occurrence + NULL last.
        let inner = union_of(&db, [null(&db), int]);
        let outer = union_of(&db, [inner, chr, int]);
        let TyKind::Union(members) = outer.kind(&db) else {
            panic!("expected a union")
        };
        assert_eq!(members, &vec![int, chr, null(&db)]);

        // Any and Unknown absorb.
        assert_eq!(union_of(&db, [int, any(&db)]), any(&db));
        assert_eq!(union_of(&db, [unknown(&db), int]), unknown(&db));

        // Singleton unwraps; empty collapses to NULL.
        assert_eq!(union_of(&db, [int, int]), int);
        assert_eq!(union_of(&db, []), null(&db));
        // `T | NULL` stays a union with NULL last.
        let optional = union_of(&db, [null(&db), chr]);
        let TyKind::Union(members) = optional.kind(&db) else {
            panic!("expected a union")
        };
        assert_eq!(members, &vec![chr, null(&db)]);
    }

    #[test]
    fn constraint_lattice_joins() {
        use Constraint::*;
        assert_eq!(Unconstrained.join(Numeric), Numeric);
        assert_eq!(Numeric.join(AtomicElement), ScalarNumeric);
        assert_eq!(ScalarNumeric.join(Numeric), ScalarNumeric);
        assert_eq!(AtomicElement.join(AtomicElement), AtomicElement);
    }
}
