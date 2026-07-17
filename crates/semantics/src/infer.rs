//! The inference substrate: a dense union-find table over interned types.
//!
//! Unification stays syntactic — the invariant floor: a union unifies only
//! with a structurally equal union (set equality after resolution), plus the
//! single `T | NULL`-vs-`U | NULL` member-wise case; all directional
//! member-wise logic lives in compatibility checking, never here. A union may
//! be *bound to* a variable, but no union constraint is ever imposed on one
//! (the HM-speed guardrail). Constraints ride on unbound entries and join
//! through the lattice on redirect.
//!
//! Probes snapshot the table and roll back completely: the entry vector
//! truncates to its snapshot length and mutated older entries revert through
//! an undo log, so a failed probe leaves no trace.

use crate::Db;
use crate::types::{Constraint, FunctionType, InferenceVar, Ty, TyKind, union_of};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry<'db> {
    Unbound { level: u32, constraint: Constraint },
    Bound(Ty<'db>),
    Redirect(InferenceVar),
}

/// Why a unification failed; the compatibility layer turns this into wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifyError<'db> {
    Mismatch(Ty<'db>, Ty<'db>),
    /// The occurs check refused an infinite type.
    Occurs(InferenceVar, Ty<'db>),
    /// A constraint rejected the bound type (e.g. numeric vs character).
    ConstraintRejected(Constraint, Ty<'db>),
}

#[derive(Debug, Clone, Copy)]
pub struct Snapshot {
    entries: usize,
    undo: usize,
}

/// The dense entry table: the id counter IS the length, so a dangling id is
/// unrepresentable.
#[derive(Debug, Default)]
pub struct InferenceTable<'db> {
    entries: Vec<Entry<'db>>,
    /// Previous values of entries mutated below a snapshot boundary.
    undo: Vec<(InferenceVar, Entry<'db>)>,
    pub level: u32,
}

impl<'db> InferenceTable<'db> {
    pub fn fresh(&mut self, constraint: Constraint) -> InferenceVar {
        let var = InferenceVar(self.entries.len() as u32);
        self.entries.push(Entry::Unbound { level: self.level, constraint });
        var
    }

    pub fn fresh_ty(&mut self, db: &'db dyn Db, constraint: Constraint) -> Ty<'db> {
        let var = self.fresh(constraint);
        Ty::new(db, TyKind::Var(var))
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot { entries: self.entries.len(), undo: self.undo.len() }
    }

    pub fn rollback(&mut self, snapshot: Snapshot) {
        while self.undo.len() > snapshot.undo {
            let (var, previous) = self.undo.pop().expect("undo length checked");
            if (var.0 as usize) < snapshot.entries {
                self.entries[var.0 as usize] = previous;
            }
        }
        self.entries.truncate(snapshot.entries);
    }

    fn set(&mut self, var: InferenceVar, entry: Entry<'db>) {
        let previous = std::mem::replace(&mut self.entries[var.0 as usize], entry);
        self.undo.push((var, previous));
    }

    /// The representative variable at the end of a redirect chain.
    pub fn find(&self, var: InferenceVar) -> InferenceVar {
        let mut current = var;
        loop {
            match &self.entries[current.0 as usize] {
                Entry::Redirect(next) => current = *next,
                _ => return current,
            }
        }
    }

    pub fn entry(&self, var: InferenceVar) -> &Entry<'db> {
        &self.entries[self.find(var).0 as usize]
    }

    /// Shallow-resolve: follow variables to their binding, without walking
    /// into structure.
    pub fn shallow_resolve(&self, db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
        let mut current = ty;
        loop {
            match current.kind(db) {
                TyKind::Var(var) => match self.entry(*var) {
                    Entry::Bound(bound) => current = *bound,
                    _ => {
                        // Canonicalize to the representative.
                        let representative = self.find(*var);
                        return if representative == *var {
                            current
                        } else {
                            Ty::new(db, TyKind::Var(representative))
                        };
                    }
                },
                _ => return current,
            }
        }
    }

    /// Deep-resolve: replace every bound variable in the structure.
    pub fn resolve(&self, db: &'db dyn Db, ty: Ty<'db>) -> Ty<'db> {
        let shallow = self.shallow_resolve(db, ty);
        match shallow.kind(db) {
            TyKind::Any
            | TyKind::Unknown
            | TyKind::Null
            | TyKind::Scalar(_)
            | TyKind::Var(_)
            | TyKind::Rigid(_) => shallow,
            TyKind::Vector(element) => {
                let element = self.resolve(db, *element);
                Ty::new(db, TyKind::Vector(element))
            }
            TyKind::NamedVector(element) => {
                let element = self.resolve(db, *element);
                Ty::new(db, TyKind::NamedVector(element))
            }
            TyKind::List(element) => {
                let element = self.resolve(db, *element);
                Ty::new(db, TyKind::List(element))
            }
            TyKind::NamedList(element) => {
                let element = self.resolve(db, *element);
                Ty::new(db, TyKind::NamedList(element))
            }
            TyKind::Tuple(items) => {
                let items = items.iter().map(|&item| self.resolve(db, item)).collect();
                Ty::new(db, TyKind::Tuple(items))
            }
            TyKind::Record(fields) => {
                let fields = fields
                    .iter()
                    .map(|field| {
                        let mut field = field.clone();
                        field.ty = self.resolve(db, field.ty);
                        field
                    })
                    .collect();
                Ty::new(db, TyKind::Record(fields))
            }
            TyKind::Function(function) => {
                let function = self.resolve_function(db, function);
                Ty::new(db, TyKind::Function(function))
            }
            TyKind::Union(members) => {
                // Members can collapse after resolution: re-normalize.
                let members: Vec<Ty<'db>> =
                    members.iter().map(|&member| self.resolve(db, member)).collect();
                union_of(db, members)
            }
            TyKind::Named(name, arguments) => {
                let arguments =
                    arguments.iter().map(|&argument| self.resolve(db, argument)).collect();
                Ty::new(db, TyKind::Named(*name, arguments))
            }
        }
    }

    fn resolve_function(&self, db: &'db dyn Db, function: &FunctionType<'db>) -> FunctionType<'db> {
        FunctionType {
            positional: function.positional.iter().map(|&ty| self.resolve(db, ty)).collect(),
            named: function
                .named
                .iter()
                .map(|field| {
                    let mut field = field.clone();
                    field.ty = self.resolve(db, field.ty);
                    field
                })
                .collect(),
            variadic: function.variadic.as_ref().map(|rest| {
                let mut rest = rest.clone();
                rest.element = self.resolve(db, rest.element);
                rest
            }),
            ret: self.resolve(db, function.ret),
        }
    }

    /// Bind `var` to `ty` after the occurs check and constraint admission.
    fn bind(&mut self, db: &'db dyn Db, var: InferenceVar, ty: Ty<'db>) -> Result<(), UnifyError<'db>> {
        let representative = self.find(var);
        if self.occurs(db, representative, ty) {
            return Err(UnifyError::Occurs(representative, ty));
        }
        let Entry::Unbound { level, constraint } = *self.entry(representative) else {
            unreachable!("bind target is always unbound after find");
        };
        if let Some(error) = constraint_rejects(db, constraint, ty) {
            return Err(error);
        }
        // Level-adjust free variables in `ty` up to this entry's level so
        // generalization at an outer level cannot quantify them away.
        self.adjust_levels(db, level, ty);
        self.set(representative, Entry::Bound(ty));
        Ok(())
    }

    fn adjust_levels(&mut self, db: &'db dyn Db, level: u32, ty: Ty<'db>) {
        let shallow = self.shallow_resolve(db, ty);
        match shallow.kind(db) {
            TyKind::Var(var) => {
                let representative = self.find(*var);
                if let Entry::Unbound { level: var_level, constraint } =
                    *self.entry(representative)
                    && var_level > level
                {
                    self.set(representative, Entry::Unbound { level, constraint });
                }
            }
            TyKind::Vector(inner)
            | TyKind::NamedVector(inner)
            | TyKind::List(inner)
            | TyKind::NamedList(inner) => self.adjust_levels(db, level, *inner),
            TyKind::Tuple(items) => {
                for &item in items.clone().iter() {
                    self.adjust_levels(db, level, item);
                }
            }
            TyKind::Record(fields) => {
                for field in fields.clone().iter() {
                    self.adjust_levels(db, level, field.ty);
                }
            }
            TyKind::Function(function) => {
                let function = function.clone();
                for &ty in &function.positional {
                    self.adjust_levels(db, level, ty);
                }
                for field in &function.named {
                    self.adjust_levels(db, level, field.ty);
                }
                if let Some(rest) = &function.variadic {
                    self.adjust_levels(db, level, rest.element);
                }
                self.adjust_levels(db, level, function.ret);
            }
            TyKind::Union(members) => {
                for &member in members.clone().iter() {
                    self.adjust_levels(db, level, member);
                }
            }
            TyKind::Named(_, arguments) => {
                for &argument in arguments.clone().iter() {
                    self.adjust_levels(db, level, argument);
                }
            }
            _ => {}
        }
    }

    fn occurs(&self, db: &'db dyn Db, var: InferenceVar, ty: Ty<'db>) -> bool {
        let shallow = self.shallow_resolve(db, ty);
        match shallow.kind(db) {
            TyKind::Var(other) => self.find(*other) == var,
            TyKind::Vector(inner)
            | TyKind::NamedVector(inner)
            | TyKind::List(inner)
            | TyKind::NamedList(inner) => self.occurs(db, var, *inner),
            TyKind::Tuple(items) => items.iter().any(|&item| self.occurs(db, var, item)),
            TyKind::Record(fields) => fields.iter().any(|field| self.occurs(db, var, field.ty)),
            TyKind::Function(function) => {
                function.positional.iter().any(|&ty| self.occurs(db, var, ty))
                    || function.named.iter().any(|field| self.occurs(db, var, field.ty))
                    || function
                        .variadic
                        .as_ref()
                        .is_some_and(|rest| self.occurs(db, var, rest.element))
                    || self.occurs(db, var, function.ret)
            }
            TyKind::Union(members) => members.iter().any(|&member| self.occurs(db, var, member)),
            TyKind::Named(_, arguments) => {
                arguments.iter().any(|&argument| self.occurs(db, var, argument))
            }
            _ => false,
        }
    }

    pub fn unify(&mut self, db: &'db dyn Db, a: Ty<'db>, b: Ty<'db>) -> Result<(), UnifyError<'db>> {
        let a = self.shallow_resolve(db, a);
        let b = self.shallow_resolve(db, b);
        if a == b {
            return Ok(());
        }
        match (a.kind(db), b.kind(db)) {
            (TyKind::Var(left), TyKind::Var(right)) => {
                let left = self.find(*left);
                let right = self.find(*right);
                if left == right {
                    return Ok(());
                }
                let Entry::Unbound { level: left_level, constraint: left_constraint } =
                    *self.entry(left)
                else {
                    unreachable!("shallow resolve leaves only unbound variables")
                };
                let Entry::Unbound { level: right_level, constraint: right_constraint } =
                    *self.entry(right)
                else {
                    unreachable!("shallow resolve leaves only unbound variables")
                };
                // Redirect the younger to the older, joining constraints and
                // keeping the minimum level.
                let (winner, loser) = if left.0 <= right.0 { (left, right) } else { (right, left) };
                self.set(
                    winner,
                    Entry::Unbound {
                        level: left_level.min(right_level),
                        constraint: left_constraint.join(right_constraint),
                    },
                );
                self.set(loser, Entry::Redirect(winner));
                Ok(())
            }
            (TyKind::Var(var), _) => self.bind(db, *var, b),
            (_, TyKind::Var(var)) => self.bind(db, *var, a),
            (TyKind::Vector(left), TyKind::Vector(right))
            | (TyKind::NamedVector(left), TyKind::NamedVector(right))
            | (TyKind::List(left), TyKind::List(right))
            | (TyKind::NamedList(left), TyKind::NamedList(right)) => {
                self.unify(db, *left, *right)
            }
            (TyKind::Tuple(left), TyKind::Tuple(right)) => {
                if left.len() != right.len() {
                    return Err(UnifyError::Mismatch(a, b));
                }
                let pairs: Vec<(Ty<'db>, Ty<'db>)> =
                    left.iter().copied().zip(right.iter().copied()).collect();
                for (left, right) in pairs {
                    self.unify(db, left, right)?;
                }
                Ok(())
            }
            (TyKind::Record(left), TyKind::Record(right)) => {
                if left.len() != right.len()
                    || left
                        .iter()
                        .zip(right.iter())
                        .any(|(l, r)| l.name != r.name || l.optional != r.optional)
                {
                    return Err(UnifyError::Mismatch(a, b));
                }
                let pairs: Vec<(Ty<'db>, Ty<'db>)> =
                    left.iter().map(|f| f.ty).zip(right.iter().map(|f| f.ty)).collect();
                for (left, right) in pairs {
                    self.unify(db, left, right)?;
                }
                Ok(())
            }
            (TyKind::Function(left), TyKind::Function(right)) => {
                if left.positional.len() != right.positional.len()
                    || left.named.len() != right.named.len()
                    || left.variadic.is_some() != right.variadic.is_some()
                    || left
                        .named
                        .iter()
                        .zip(right.named.iter())
                        .any(|(l, r)| l.name != r.name || l.optional != r.optional)
                {
                    return Err(UnifyError::Mismatch(a, b));
                }
                let left = left.clone();
                let right = right.clone();
                for (l, r) in left.positional.iter().zip(right.positional.iter()) {
                    self.unify(db, *l, *r)?;
                }
                for (l, r) in left.named.iter().zip(right.named.iter()) {
                    self.unify(db, l.ty, r.ty)?;
                }
                if let (Some(l), Some(r)) = (&left.variadic, &right.variadic) {
                    self.unify(db, l.element, r.element)?;
                }
                self.unify(db, left.ret, right.ret)
            }
            (TyKind::Named(left_name, left_arguments), TyKind::Named(right_name, right_arguments)) => {
                if left_name != right_name || left_arguments.len() != right_arguments.len() {
                    return Err(UnifyError::Mismatch(a, b));
                }
                let pairs: Vec<(Ty<'db>, Ty<'db>)> = left_arguments
                    .iter()
                    .copied()
                    .zip(right_arguments.iter().copied())
                    .collect();
                for (left, right) in pairs {
                    self.unify(db, left, right)?;
                }
                Ok(())
            }
            (TyKind::Union(_), TyKind::Union(_)) => self.unify_unions(db, a, b),
            _ => Err(UnifyError::Mismatch(a, b)),
        }
    }

    /// Unions unify by set equality after resolution, plus the single
    /// `T | NULL` vs `U | NULL` member-wise case (both two-member nullable
    /// unions: unify the non-NULL members).
    fn unify_unions(&mut self, db: &'db dyn Db, a: Ty<'db>, b: Ty<'db>) -> Result<(), UnifyError<'db>> {
        let resolved_a = self.resolve(db, a);
        let resolved_b = self.resolve(db, b);
        if resolved_a == resolved_b {
            return Ok(());
        }
        let (TyKind::Union(left), TyKind::Union(right)) = (resolved_a.kind(db), resolved_b.kind(db))
        else {
            // Resolution collapsed one side; retry the general path.
            return self.unify(db, resolved_a, resolved_b);
        };
        if let (Some(left_inner), Some(right_inner)) =
            (nullable_single_member(db, left), nullable_single_member(db, right))
        {
            return self.unify(db, left_inner, right_inner);
        }
        if left.len() == right.len() && left.iter().all(|member| right.contains(member)) {
            return Ok(());
        }
        Err(UnifyError::Mismatch(a, b))
    }
}

/// `T | NULL` (exactly two members, one NULL) yields `T`.
fn nullable_single_member<'db>(db: &'db dyn Db, members: &[Ty<'db>]) -> Option<Ty<'db>> {
    if members.len() != 2 {
        return None;
    }
    let null_at = members.iter().position(|member| matches!(member.kind(db), TyKind::Null))?;
    Some(members[1 - null_at])
}

fn constraint_rejects<'db>(
    db: &'db dyn Db,
    constraint: Constraint,
    ty: Ty<'db>,
) -> Option<UnifyError<'db>> {
    use crate::types::Atomic;
    let admissible = match constraint {
        Constraint::Unconstrained => true,
        Constraint::Numeric => matches!(
            ty.kind(db),
            TyKind::Scalar(Atomic::Integer | Atomic::Double)
                | TyKind::Vector(_)
                | TyKind::Any
                | TyKind::Unknown
        ),
        Constraint::AtomicElement => matches!(
            ty.kind(db),
            TyKind::Scalar(_) | TyKind::Any | TyKind::Unknown
        ),
        Constraint::ScalarNumeric => matches!(
            ty.kind(db),
            TyKind::Scalar(Atomic::Integer | Atomic::Double) | TyKind::Any | TyKind::Unknown
        ),
    };
    (!admissible).then_some(UnifyError::ConstraintRejected(constraint, ty))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RootDatabase;
    use crate::types::{Atomic, scalar};

    #[test]
    fn unify_binds_and_resolves() {
        let db = RootDatabase::default();
        let mut table = InferenceTable::default();
        let var = table.fresh_ty(&db, Constraint::Unconstrained);
        let int = scalar(&db, Atomic::Integer);
        table.unify(&db, var, int).expect("binds");
        assert_eq!(table.resolve(&db, var), int);
    }

    #[test]
    fn occurs_check_refuses_infinite_types() {
        let db = RootDatabase::default();
        let mut table = InferenceTable::default();
        let var = table.fresh(Constraint::Unconstrained);
        let var_ty = Ty::new(&db, TyKind::Var(var));
        let list = Ty::new(&db, TyKind::List(var_ty));
        assert!(matches!(table.unify(&db, var_ty, list), Err(UnifyError::Occurs(..))));
    }

    #[test]
    fn constraint_joins_through_redirects() {
        let db = RootDatabase::default();
        let mut table = InferenceTable::default();
        let a = table.fresh(Constraint::Numeric);
        let b = table.fresh(Constraint::AtomicElement);
        let a_ty = Ty::new(&db, TyKind::Var(a));
        let b_ty = Ty::new(&db, TyKind::Var(b));
        table.unify(&db, a_ty, b_ty).expect("redirects");
        let Entry::Unbound { constraint, .. } = *table.entry(a) else {
            panic!("representative stays unbound")
        };
        assert_eq!(constraint, Constraint::ScalarNumeric);
        // A character scalar now violates the joined constraint.
        let chr = scalar(&db, Atomic::Character);
        assert!(table.unify(&db, a_ty, chr).is_err());
    }

    #[test]
    fn rollback_reverts_probes_completely() {
        let db = RootDatabase::default();
        let mut table = InferenceTable::default();
        let outer = table.fresh_ty(&db, Constraint::Unconstrained);
        let snapshot = table.snapshot();
        let probe = table.fresh_ty(&db, Constraint::Unconstrained);
        table.unify(&db, outer, probe).expect("unifies");
        table.unify(&db, probe, scalar(&db, Atomic::Double)).expect("binds");
        assert_eq!(table.resolve(&db, outer), scalar(&db, Atomic::Double));
        table.rollback(snapshot);
        // The pre-probe variable is unbound again.
        assert!(matches!(table.entry(InferenceVar(0)), Entry::Unbound { .. }));
        assert!(matches!(table.resolve(&db, outer).kind(&db), TyKind::Var(_)));
    }

    #[test]
    fn unions_unify_by_set_equality_and_nullable_case() {
        let db = RootDatabase::default();
        let mut table = InferenceTable::default();
        let int = scalar(&db, Atomic::Integer);
        let chr = scalar(&db, Atomic::Character);
        let null = crate::types::null(&db);
        let a = union_of(&db, [int, chr]);
        let b = union_of(&db, [chr, int]);
        assert!(table.unify(&db, a, b).is_ok());

        // `T | NULL` vs `integer | NULL` binds T to integer.
        let var = table.fresh_ty(&db, Constraint::Unconstrained);
        let left = union_of(&db, [var, null]);
        let right = union_of(&db, [int, null]);
        table.unify(&db, left, right).expect("nullable member-wise case");
        assert_eq!(table.resolve(&db, var), int);

        // Distinct non-nullable unions refuse.
        let c = union_of(&db, [int, null]);
        let d = union_of(&db, [chr, null]);
        assert!(table.unify(&db, c, d).is_err());
    }
}
