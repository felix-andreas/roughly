//! Naming: the R variable model — mutable slots with reaching-write flow.
//!
//! Every `<-`/`=` of a name in a frame resolves to one `BindingId` slot (R
//! assignment mutates the scope's environment); `<<-` walks the lexical chain
//! to the nearest enclosing slot. A read resolves to a slot only when a write
//! can reach it — an unassignable slot does not shadow, so lookup falls
//! outward exactly like R's runtime — and is flagged maybe-undefined when an
//! unassigned path also reaches. The unused check reports **assignment sites**
//! whose write no read reaches. Control flow forks and joins per-slot
//! reaching-write sets; loop bodies iterate to a fixed point with binding
//! creation made idempotent by site (re-walks mint no new ids) and effects
//! recorded only on the final pass.
//!
//! Data-masked evaluation (NSE) is recognized structurally: a `[` carrying an
//! unambiguous data.table signature masks its index arguments (recorded in
//! `masked_subsets`), and the base masking family (`with`, `within`,
//! `subset`, `transform`) masks every argument after the data. Masked reads
//! that resolve keep their ordinary resolution; ones that do not stay quiet —
//! column references are not unresolved names.
//!
//! Deferred (structure already carries them): the nested-loop region memo and
//! the unused-parameter lint.

use crate::hir::{Argument, AssignSpelling, ExprId, ExpressionKind, Module, UnaryOperator};
use rustc_hash::FxHashMap;
use std::collections::{BTreeMap, BTreeSet};
use syntax::TextRange;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BindingId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingInfo {
    pub id: BindingId,
    pub name: String,
    /// The first write's site — the slot's definition site for hover/goto.
    pub range: TextRange,
    pub kind: BindingKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// The item's own top-level binding (package-visible; never unused).
    TopLevel,
    /// A variable slot in a function or block scope.
    Local,
    /// A function parameter's slot (the parameter itself is never reported).
    Parameter,
    /// A `for (v in …)` loop variable (often intentionally unused).
    ForVariable,
}

/// A dead store: an assignment whose written value no read reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusedAssignment {
    pub name: String,
    pub range: TextRange,
}

/// A qualified `pkg::name` / `pkg:::name` read, recorded for validation
/// against the stub corpus's per-namespace exports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceRead {
    pub internal: bool,
    pub package: String,
    pub name: Option<String>,
}

/// The naming facts of one item, item-relative like all HIR spans.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemNaming {
    pub bindings: BTreeMap<BindingId, BindingInfo>,
    /// Reads and assignment targets resolved to their slot.
    pub resolutions: BTreeMap<ExprId, BindingId>,
    /// Reads an unassigned path can reach.
    pub maybe_undefined: BTreeSet<ExprId>,
    /// Reads no lexical slot resolves — package globals, stubs, or unresolved.
    pub non_locals: BTreeMap<ExprId, String>,
    /// The subset of `non_locals` read from inside a nested function: the
    /// read happens when the function is *called*, after the enclosing frame
    /// finished executing.
    pub deferred_non_locals: BTreeSet<ExprId>,
    /// Dead stores (surfaced only when the unused check is enabled).
    pub unused_assignments: Vec<UnusedAssignment>,
    /// Slots read or super-assigned from inside a function nested below the
    /// slot's frame: every write to them stays observable.
    pub captured_slots: BTreeSet<BindingId>,
    /// `[` expressions recognized as data.table syntax: their indexes
    /// evaluate in the data's frame, and the whole bracket types `Unknown`.
    pub masked_subsets: BTreeSet<ExprId>,
    /// Qualified reads, skipping quieted (opaque-construct) contexts.
    pub namespace_reads: BTreeMap<ExprId, NamespaceRead>,
    /// Names written by `<<-` with no enclosing binding: R creates them in
    /// the global environment, so they resolve package-wide.
    pub super_globals: BTreeSet<String>,
}

/// Resolve one item's HIR. The item's own top-level assignment target (if any)
/// becomes a `TopLevel` binding; everything below follows the slot model.
pub fn resolve_item(module: &Module) -> ItemNaming {
    let mut context = Context {
        module,
        next_binding: 0,
        scopes: vec![Scope::new(ScopeKind::TopLevel)],
        bindings_by_site: FxHashMap::default(),
        flow: FlowState::new(),
        writes: Vec::new(),
        write_by_expression: FxHashMap::default(),
        emit: true,
        quiet_depth: 0,
        naming: ItemNaming::default(),
    };
    if let Some(root) = module.root {
        context.resolve(root);
    }
    context.collect_unused();
    context.naming
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    /// A function body's frame: executes when *called*, so cross-frame reads
    /// from below are captures.
    Function,
    /// A `local({...})` block's environment: writes bind here and vanish with
    /// it, but execution is immediate, so it is not a capture boundary.
    Local,
    /// The item's top level.
    TopLevel,
}

struct Scope {
    kind: ScopeKind,
    slots: BTreeMap<String, BindingId>,
}

impl Scope {
    fn new(kind: ScopeKind) -> Scope {
        Scope {
            kind,
            slots: BTreeMap::new(),
        }
    }
}

/// One element of a slot's reaching-write set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reach {
    /// Some path reaches here without any write.
    Unassigned,
    /// A write with no reportable site (parameter, for-variable).
    Implicit,
    /// An assignment write, by index into `writes`.
    Assignment(u32),
}

type FlowState = BTreeMap<BindingId, BTreeSet<Reach>>;

struct AssignmentWrite {
    name: String,
    range: TextRange,
    used: bool,
    reportable: bool,
}

struct Context<'a> {
    module: &'a Module,
    next_binding: u32,
    scopes: Vec<Scope>,
    /// Loop re-walks must mint no new ids: one binding per (site, name).
    bindings_by_site: FxHashMap<(u32, u32, String), BindingId>,
    flow: FlowState,
    writes: Vec<AssignmentWrite>,
    write_by_expression: FxHashMap<ExprId, u32>,
    /// Diagnostic-bearing sets are recorded only on a loop region's final walk
    /// (used-marking is monotone and always on).
    emit: bool,
    /// Non-zero inside an unsupported operator's operands: reads resolve but
    /// unresolved names stay quiet.
    quiet_depth: u32,
    naming: ItemNaming,
}

impl Context<'_> {
    fn mint_binding(&mut self, name: &str, range: TextRange, kind: BindingKind) -> BindingId {
        let key = (
            u32::from(range.start()),
            u32::from(range.end()),
            name.to_owned(),
        );
        if let Some(&existing) = self.bindings_by_site.get(&key) {
            return existing;
        }
        let id = BindingId(self.next_binding);
        self.next_binding += 1;
        self.bindings_by_site.insert(key, id);
        self.naming.bindings.insert(
            id,
            BindingInfo {
                id,
                name: name.to_owned(),
                range,
                kind,
            },
        );
        id
    }

    fn current_function_depth(&self) -> usize {
        self.scopes
            .iter()
            .rposition(|scope| scope.kind == ScopeKind::Function)
            .unwrap_or(0)
    }

    fn record_write(&mut self, slot: BindingId, reach: Reach) {
        let set = self.flow.entry(slot).or_default();
        set.clear();
        set.insert(reach);
    }

    fn resolve_many(&mut self, ids: &[ExprId]) {
        for &id in ids {
            self.resolve(id);
        }
    }

    fn resolve(&mut self, id: ExprId) {
        let expression = self.module.expression(id).clone();
        match &expression.kind {
            ExpressionKind::Missing | ExpressionKind::Break | ExpressionKind::Next => {}
            ExpressionKind::Literal(_) => {}
            ExpressionKind::NameRef(name) => self.resolve_read(id, name),
            ExpressionKind::Assign {
                spelling,
                target,
                value,
            } => {
                // Value first: `x <- x + 1` reads the previous state. A
                // replacement-form target instead resolves first — its base
                // binds before the value is examined, so the value's reads
                // of the same name land on the fresh binding
                // (`self$i <- self$i + 1` reports `self` once, at the
                // target).
                let name_target = matches!(
                    self.module.expression(*target).kind,
                    ExpressionKind::NameRef(_)
                );
                if name_target {
                    self.resolve(*value);
                    self.resolve_assignment_target(id, *target, *spelling);
                } else {
                    self.resolve_assignment_target(id, *target, *spelling);
                    self.resolve(*value);
                }
            }
            ExpressionKind::Unary { operand, operator } => {
                // A formula quotes its operand: names inside are model syntax.
                if *operator != UnaryOperator::Tilde && *operator != UnaryOperator::Help {
                    self.resolve(*operand);
                }
            }
            ExpressionKind::Binary { operator, lhs, rhs } => {
                use crate::hir::BinaryOperator;
                match operator {
                    // A formula quotes its operands: names inside are model
                    // syntax, exactly like the unary form.
                    BinaryOperator::Tilde | BinaryOperator::Help => {}
                    // Operands of operators the checker does not model
                    // (`&`/`|`, `|>`, user `%op%`s) still resolve — the IDE
                    // needs goto/references inside pipes — but a non-local
                    // read there stays quiet: the construct is opaque, so an
                    // unresolved name in it is not a reportable finding.
                    BinaryOperator::And
                    | BinaryOperator::Or
                    | BinaryOperator::Pipe
                    | BinaryOperator::Special => {
                        self.quiet_depth += 1;
                        self.resolve(*lhs);
                        self.resolve(*rhs);
                        self.quiet_depth -= 1;
                    }
                    _ => {
                        self.resolve(*lhs);
                        self.resolve(*rhs);
                    }
                }
            }
            ExpressionKind::Call { callee, arguments } => {
                self.resolve(*callee);
                // The base masking family evaluates every argument after the
                // data inside the data's frame; a locally defined function of
                // the same name masks nothing.
                let masking_family = matches!(
                    &self.module.expression(*callee).kind,
                    ExpressionKind::NameRef(name)
                        if matches!(name.as_str(), "with" | "within" | "subset" | "transform")
                ) && !self.naming.resolutions.contains_key(callee);
                if masking_family {
                    let mut argument_iter = arguments.iter();
                    if let Some(data) = argument_iter.next()
                        && let Some(value) = data.value
                    {
                        self.resolve(value);
                    }
                    self.quiet_depth += 1;
                    for argument in argument_iter {
                        if let Some(value) = argument.value {
                            self.resolve(value);
                        }
                    }
                    self.quiet_depth -= 1;
                } else {
                    self.resolve_arguments(arguments);
                }
            }
            ExpressionKind::Index {
                double,
                target,
                arguments,
            } => {
                self.resolve(*target);
                if !*double && self.data_table_signature(arguments) {
                    self.naming.masked_subsets.insert(id);
                    self.quiet_depth += 1;
                    self.resolve_arguments(arguments);
                    self.quiet_depth -= 1;
                } else {
                    self.resolve_arguments(arguments);
                }
            }
            ExpressionKind::Field { target, .. } => self.resolve(*target),
            ExpressionKind::Namespace {
                internal,
                package,
                name,
            } => {
                if self.emit
                    && self.quiet_depth == 0
                    && let Some(package) = package
                {
                    self.naming.namespace_reads.insert(
                        id,
                        NamespaceRead {
                            internal: *internal,
                            package: package.clone(),
                            name: name.clone(),
                        },
                    );
                }
            }
            ExpressionKind::Function { parameters, body } => {
                self.scopes.push(Scope::new(ScopeKind::Function));
                let saved_flow = std::mem::take(&mut self.flow);
                let parameters = parameters.clone();
                for parameter in &parameters {
                    let slot =
                        self.mint_binding(&parameter.name, parameter.range, BindingKind::Parameter);
                    self.scopes
                        .last_mut()
                        .expect("scope stack is never empty")
                        .slots
                        .insert(parameter.name.clone(), slot);
                    self.record_write(slot, Reach::Implicit);
                }
                self.premint_frame_assignments(*body);
                for parameter in &parameters {
                    if let Some(default) = parameter.default {
                        self.resolve(default);
                    }
                }
                self.resolve(*body);
                self.scopes.pop();
                self.flow = saved_flow;
            }
            ExpressionKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.resolve(*condition);
                let entry = self.flow.clone();
                self.resolve(*then_branch);
                let after_then = std::mem::replace(&mut self.flow, entry);
                if let Some(else_branch) = else_branch {
                    self.resolve(*else_branch);
                }
                join_flow(&mut self.flow, &after_then);
            }
            ExpressionKind::For {
                variable,
                variable_range,
                sequence,
                body,
            } => {
                self.resolve(*sequence);
                if let (Some(variable), Some(range)) = (variable, variable_range) {
                    let slot = self.mint_binding(variable, *range, BindingKind::ForVariable);
                    self.scopes
                        .last_mut()
                        .expect("scope stack is never empty")
                        .slots
                        .insert(variable.clone(), slot);
                    self.record_write(slot, Reach::Implicit);
                }
                self.loop_body(*body, true);
            }
            ExpressionKind::While { condition, body } => {
                self.resolve(*condition);
                self.loop_body(*body, true);
            }
            ExpressionKind::Repeat { body } => {
                // `repeat` runs at least once: the exit state applies without
                // joining the never-entered path.
                self.loop_body(*body, false);
            }
            ExpressionKind::Local { body } => {
                // Reads fall through to enclosing scopes as ordinary reads
                // and control flow runs straight through; only the bindings
                // are scoped.
                self.scopes.push(Scope::new(ScopeKind::Local));
                self.premint_frame_assignments(*body);
                self.resolve(*body);
                self.scopes.pop();
            }
            ExpressionKind::Block { statements, .. } => {
                let statements = statements.clone();
                self.resolve_many(&statements);
            }
            ExpressionKind::Paren(inner) => self.resolve(*inner),
        }
    }

    fn resolve_arguments(&mut self, arguments: &[Argument]) {
        for argument in arguments.iter() {
            if let Some(value) = argument.value {
                self.resolve(value);
            }
        }
    }

    /// A `[` bracket carrying an unambiguous data.table signature: a `by =` /
    /// `keyby =` argument, a `:=` column assignment, a `.()` list call, or a
    /// `.SD`-family special anywhere in its index arguments.
    fn data_table_signature(&self, arguments: &[Argument]) -> bool {
        arguments.iter().any(|argument| {
            if matches!(argument.name.as_deref(), Some("by") | Some("keyby")) {
                return true;
            }
            argument
                .value
                .is_some_and(|value| self.data_table_marker(value))
        })
    }

    fn data_table_marker(&self, id: ExprId) -> bool {
        let kind = &self.module.expression(id).kind;
        match kind {
            ExpressionKind::Assign {
                spelling: AssignSpelling::Walrus,
                ..
            } => true,
            ExpressionKind::NameRef(name) => {
                matches!(
                    name.as_str(),
                    ".SD" | ".N" | ".I" | ".BY" | ".GRP" | ".EACHI"
                )
            }
            ExpressionKind::Call { callee, .. }
                if matches!(
                    &self.module.expression(*callee).kind,
                    ExpressionKind::NameRef(name) if name == "."
                ) =>
            {
                true
            }
            // Markers can sit anywhere inside the index expressions, but a
            // nested function body is its own evaluation context.
            ExpressionKind::Function { .. } => false,
            _ => kind
                .child_ids()
                .iter()
                .any(|&child| self.data_table_marker(child)),
        }
    }

    /// A loop body re-walks to a fixed point (the reach lattice is finite and
    /// joins only grow); effects are recorded only on the converged final
    /// pass. `may_skip` joins the never-entered path back in (`for`/`while`;
    /// `repeat` runs at least once).
    fn loop_body(&mut self, body: ExprId, may_skip: bool) {
        let entry = self.flow.clone();
        let saved_emit = self.emit;
        self.emit = false;
        loop {
            let before = self.flow.clone();
            self.resolve(body);
            join_flow(&mut self.flow, &before);
            if self.flow == before {
                break;
            }
        }
        self.emit = saved_emit;
        // Final pass over the converged state records diagnostics once.
        let converged = self.flow.clone();
        self.resolve(body);
        join_flow(&mut self.flow, &converged);
        if may_skip {
            join_flow(&mut self.flow, &entry);
        }
    }

    /// Pre-mint slots for every direct assignment target (and `for` variable)
    /// of a frame's body, so a closure defined earlier in the frame can
    /// resolve a name written later — the closure runs after the frame has
    /// executed. Nested function and `local()` bodies bind their own frames.
    fn premint_frame_assignments(&mut self, id: ExprId) {
        let kind = self.module.expression(id).kind.clone();
        match &kind {
            ExpressionKind::Function { .. } | ExpressionKind::Local { .. } => {}
            ExpressionKind::Assign {
                spelling,
                target,
                value,
            } => {
                if *spelling != AssignSpelling::Super
                    && let ExpressionKind::NameRef(name) = &self.module.expression(*target).kind
                {
                    let range = self.module.expression(*target).range;
                    self.premint_slot(name, range, BindingKind::Local);
                }
                self.premint_frame_assignments(*value);
            }
            ExpressionKind::For {
                variable,
                variable_range,
                sequence,
                body,
            } => {
                if let (Some(variable), Some(range)) = (variable, variable_range) {
                    self.premint_slot(variable, *range, BindingKind::ForVariable);
                }
                self.premint_frame_assignments(*sequence);
                self.premint_frame_assignments(*body);
            }
            _ => {
                for child in kind.child_ids() {
                    self.premint_frame_assignments(child);
                }
            }
        }
    }

    fn premint_slot(&mut self, name: &str, range: TextRange, kind: BindingKind) {
        if self
            .scopes
            .last()
            .expect("scope stack is never empty")
            .slots
            .contains_key(name)
        {
            return;
        }
        let slot = self.mint_binding(name, range, kind);
        self.scopes
            .last_mut()
            .expect("scope stack is never empty")
            .slots
            .insert(name.to_owned(), slot);
    }

    fn resolve_read(&mut self, id: ExprId, name: &str) {
        if name == "..." || name.starts_with("..") && name[2..].chars().all(|c| c.is_ascii_digit())
        {
            return;
        }
        // A slot resolves when a write can reach the read, or when the read
        // crosses a function boundary (a capture: the closure runs after the
        // frame has executed, so every frame write is observable). An
        // unassignable slot does not shadow — the lookup falls outward,
        // exactly like R's runtime.
        let function_depth = self.current_function_depth();
        let resolved = self
            .scopes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(depth, scope)| {
                let &slot = scope.slots.get(name)?;
                (self.flow.contains_key(&slot) || depth < function_depth).then_some((depth, slot))
            });
        match resolved {
            Some((depth, slot)) => {
                self.naming.resolutions.insert(id, slot);
                // Mark every reaching write used; an unassigned reaching path
                // makes the read maybe-undefined.
                if let Some(reaches) = self.flow.get(&slot) {
                    let reaches: Vec<Reach> = reaches.iter().copied().collect();
                    let mut maybe_undefined = false;
                    for reach in reaches {
                        match reach {
                            Reach::Assignment(index) => {
                                self.writes[index as usize].used = true;
                            }
                            Reach::Unassigned => maybe_undefined = true,
                            Reach::Implicit => {}
                        }
                    }
                    if maybe_undefined && self.emit {
                        self.naming.maybe_undefined.insert(id);
                    }
                }
                if depth < self.current_function_depth() {
                    // A read of an enclosing frame's slot from inside a
                    // function: a capture — every write to it stays
                    // observable, so it is never a dead store.
                    self.naming.captured_slots.insert(slot);
                    for index in 0..self.writes.len() {
                        if self.writes[index].name == *name {
                            self.writes[index].used = true;
                        }
                    }
                }
            }
            None => {
                if self.emit && self.quiet_depth == 0 {
                    self.naming.non_locals.insert(id, name.to_owned());
                    if self.current_function_depth() > 0 {
                        self.naming.deferred_non_locals.insert(id);
                    }
                }
            }
        }
    }

    fn resolve_assignment_target(
        &mut self,
        assignment: ExprId,
        target: ExprId,
        spelling: AssignSpelling,
    ) {
        // A masked `:=` assigns a data.table column, not a variable: the
        // target is a (quiet) column reference, never a binding.
        if spelling == AssignSpelling::Walrus && self.quiet_depth > 0 {
            self.resolve(target);
            return;
        }
        let target_expression = self.module.expression(target).clone();
        match &target_expression.kind {
            ExpressionKind::NameRef(name) => {
                let range = target_expression.range;
                match spelling {
                    AssignSpelling::Super => {
                        // `<<-` binds to the nearest enclosing slot; without
                        // one it targets the global environment (non-local).
                        let enclosing = {
                            let function_depth = self.current_function_depth();
                            (0..function_depth)
                                .rev()
                                .find_map(|depth| self.scopes[depth].slots.get(name).copied())
                        };
                        match enclosing {
                            Some(slot) => {
                                self.naming.resolutions.insert(target, slot);
                                self.naming.captured_slots.insert(slot);
                                self.record_write_site(assignment, target, name, range, slot, true);
                            }
                            None => {
                                if self.emit {
                                    self.naming.non_locals.insert(target, name.clone());
                                    self.naming.super_globals.insert(name.clone());
                                }
                            }
                        }
                    }
                    _ => {
                        let scope_kind =
                            self.scopes.last().expect("scope stack is never empty").kind;
                        let kind = match scope_kind {
                            ScopeKind::TopLevel => BindingKind::TopLevel,
                            ScopeKind::Function | ScopeKind::Local => BindingKind::Local,
                        };
                        let slot = match self
                            .scopes
                            .last()
                            .expect("scope stack is never empty")
                            .slots
                            .get(name)
                        {
                            Some(&slot) => slot,
                            None => {
                                let slot = self.mint_binding(name, range, kind);
                                self.scopes
                                    .last_mut()
                                    .expect("scope stack is never empty")
                                    .slots
                                    .insert(name.clone(), slot);
                                slot
                            }
                        };
                        self.naming.resolutions.insert(target, slot);
                        let reportable = kind == BindingKind::Local;
                        self.record_write_site(assignment, target, name, range, slot, reportable);
                    }
                }
            }
            // Replacement forms (`attr(x, "a") <- v`, `x$field <- v`)
            // read their target's base as a value — an unresolved base is a
            // reportable read — and then rebind it in the current scope
            // (R evaluates `x[i] <- v` as `x <- \`[<-\`(x, i, v)`), so
            // occurrences after the first write resolve silently.
            _ => match self.replacement_base(target) {
                Some((base_expression, name, range)) => {
                    self.resolve(base_expression);
                    let scope_kind = self.scopes.last().expect("scope stack is never empty").kind;
                    let kind = match scope_kind {
                        ScopeKind::TopLevel => BindingKind::TopLevel,
                        ScopeKind::Function | ScopeKind::Local => BindingKind::Local,
                    };
                    let current = self
                        .scopes
                        .last()
                        .expect("scope stack is never empty")
                        .slots
                        .get(&name)
                        .copied();
                    let slot = match current {
                        Some(slot) => slot,
                        None => {
                            let slot = self.mint_binding(&name, range, kind);
                            self.scopes
                                .last_mut()
                                .expect("scope stack is never empty")
                                .slots
                                .insert(name.clone(), slot);
                            slot
                        }
                    };
                    self.naming.resolutions.insert(base_expression, slot);
                    self.record_write_site(assignment, base_expression, &name, range, slot, false);
                    self.resolve_replacement_subscripts(target, base_expression);
                }
                None => self.resolve(target),
            },
        }
    }

    /// The base `NameRef` at the bottom of a replacement target chain
    /// (`x` in `attr(x$f, "a")[i] <- v`).
    fn replacement_base(&self, target: ExprId) -> Option<(ExprId, String, TextRange)> {
        let expression = self.module.expression(target);
        match &expression.kind {
            ExpressionKind::NameRef(name) => Some((target, name.clone(), expression.range)),
            ExpressionKind::Field { target: inner, .. }
            | ExpressionKind::Index { target: inner, .. } => self.replacement_base(*inner),
            ExpressionKind::Call { arguments, .. } => arguments
                .first()
                .and_then(|argument| argument.value)
                .and_then(|value| self.replacement_base(value)),
            _ => None,
        }
    }

    /// Resolves everything in a replacement target except the (already
    /// handled) base name: indexes, further call arguments, callees.
    fn resolve_replacement_subscripts(&mut self, target: ExprId, base: ExprId) {
        if target == base {
            return;
        }
        let expression = self.module.expression(target).clone();
        match &expression.kind {
            ExpressionKind::Field { target: inner, .. } => {
                self.resolve_replacement_subscripts(*inner, base);
            }
            ExpressionKind::Index {
                target: inner,
                arguments,
                ..
            } => {
                self.resolve_replacement_subscripts(*inner, base);
                for argument in arguments {
                    if let Some(value) = argument.value {
                        self.resolve(value);
                    }
                }
            }
            ExpressionKind::Call { callee, arguments } => {
                self.resolve(*callee);
                let mut arguments = arguments.iter();
                if let Some(first) = arguments.next()
                    && let Some(value) = first.value
                {
                    self.resolve_replacement_subscripts(value, base);
                }
                for argument in arguments {
                    if let Some(value) = argument.value {
                        self.resolve(value);
                    }
                }
            }
            _ => self.resolve(target),
        }
    }

    fn record_write_site(
        &mut self,
        assignment: ExprId,
        _target: ExprId,
        name: &str,
        range: TextRange,
        slot: BindingId,
        reportable: bool,
    ) {
        let index = match self.write_by_expression.get(&assignment) {
            Some(&index) => index,
            None => {
                let index = self.writes.len() as u32;
                self.writes.push(AssignmentWrite {
                    name: name.to_owned(),
                    range,
                    used: false,
                    reportable,
                });
                self.write_by_expression.insert(assignment, index);
                index
            }
        };
        // A write to a captured slot stays observable through the closure.
        if self.naming.captured_slots.contains(&slot) {
            self.writes[index as usize].used = true;
        }
        self.record_write(slot, Reach::Assignment(index));
    }

    fn collect_unused(&mut self) {
        for write in &self.writes {
            if write.reportable && !write.used {
                self.naming.unused_assignments.push(UnusedAssignment {
                    name: write.name.clone(),
                    range: write.range,
                });
            }
        }
    }
}

/// Join two control-flow paths: union the reaching sets slot-wise. A slot
/// present on only one path was unassigned on the other, so the missing side
/// contributes `Unassigned`.
fn join_flow(into: &mut FlowState, other: &FlowState) {
    for (slot, reaches) in other {
        match into.entry(*slot) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().extend(reaches.iter().copied());
            }
            std::collections::btree_map::Entry::Vacant(vacant) => {
                let mut set = reaches.clone();
                set.insert(Reach::Unassigned);
                vacant.insert(set);
            }
        }
    }
    for (slot, set) in into.iter_mut() {
        if !other.contains_key(slot) {
            set.insert(Reach::Unassigned);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::lower_item;

    fn naming_of(source: &str) -> ItemNaming {
        let parse = syntax::parse(source);
        let root = parse.syntax_node();
        let item = root.children().next().expect("one top-level item");
        let module = lower_item(&item);
        resolve_item(&module)
    }

    #[test]
    fn conditional_write_joins_and_read_resolves() {
        let naming = naming_of("f <- function(flag) {\n  x <- 1\n  if (flag) x <- 2\n  x\n}");
        // Both writes reach the final read: neither is a dead store.
        assert!(naming.unused_assignments.is_empty());
        assert!(naming.maybe_undefined.is_empty());
    }

    #[test]
    fn dead_store_is_reported() {
        let naming = naming_of("f <- function() {\n  x <- 1\n  x <- 2\n  x\n}");
        assert_eq!(naming.unused_assignments.len(), 1);
    }

    #[test]
    fn loop_accumulator_is_not_a_dead_store() {
        let naming = naming_of(
            "f <- function(n) {\n  total <- 0\n  for (i in 1:n) total <- total + i\n  total\n}",
        );
        assert!(
            naming.unused_assignments.is_empty(),
            "loop accumulator writes reach across iterations: {:?}",
            naming.unused_assignments
        );
    }

    #[test]
    fn maybe_undefined_on_partial_paths() {
        let naming = naming_of("f <- function(flag) {\n  if (flag) y <- 1\n  y\n}");
        assert_eq!(naming.maybe_undefined.len(), 1);
    }

    #[test]
    fn unresolved_reads_are_non_local() {
        let naming = naming_of("f <- function() unknown_helper(1)");
        assert!(
            naming
                .non_locals
                .values()
                .any(|name| name == "unknown_helper")
        );
    }
}
