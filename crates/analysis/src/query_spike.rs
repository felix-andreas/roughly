//! Shadow prototype (de-risking spike): a minimal salsa-style red-green memo engine wired over one
//! file-local phase chain `parse(file)` -> `lower(file)` -> `local_naming(file)`. The query bodies call
//! the real production functions (`Document::parse`, `lower`, `resolve_document_locally`), so the spike
//! measures *framework overhead and ergonomics*, not a rewrite.
//!
//! Additive and gated behind the off-by-default `query-spike` feature: it is never compiled into normal
//! or release builds and changes no production code path. It lives inside the crate so it can call the
//! crate-internal phase functions directly (all three already happen to be `pub`).
//!
//! The red-green core, in one paragraph: a global `revision` counter is bumped on every `set_input`,
//! which records the input's `changed_at` revision. Each derived query caches a `Memo { value,
//! verified_at, changed_at, deps }`. Dependencies are recorded at *runtime* — an execution stack of
//! frames collects every input/query a body reads while it runs, so no dependency is hand-declared. On
//! fetch we validate red-green: if `verified_at == revision` the memo is green (return cached); else we
//! deep-validate the recorded deps and, if none changed after our `verified_at`, bump `verified_at` to
//! the current revision and return the cached value (early cutoff); otherwise we re-execute the body and,
//! if the new value equals the old (value-eq), keep the old `changed_at` so the change does not propagate
//! (cutoff propagation), else set `changed_at = revision`.

use {
    crate::{
        document::{Document, DocumentId},
        hir::Module,
        lower::{LoweringContext, lower},
        naming::{DocumentKind, DocumentNamingComputation, resolve_document_locally},
        tree::new_parser,
    },
    std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        rc::Rc,
    },
    tree_sitter::Parser,
};

pub type Revision = u32;
pub type FileId = u32;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum QueryKind {
    // The source text and the document kind are *separate* fine-grained inputs: the naming query reads
    // the kind but not the text, so a text-only edit must not invalidate it through a kind read.
    SourceText,
    DocumentKind,
    Parse,
    Lower,
    LocalNaming,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct QueryKey {
    pub kind: QueryKind,
    pub file: FileId,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct ExecCounts {
    pub parse: u64,
    pub lower: u64,
    pub naming: u64,
}

// The memo engine ("database"). All mutable state sits behind `Cell`/`RefCell` so query bodies recurse
// through `&self` (mirroring salsa's `&db` snapshot), which is what makes runtime dependency recording
// natural: a body simply calls `self.fetch_*`, and the fetch registers itself on the current frame.
pub struct Engine {
    revision: Cell<Revision>,
    inputs: RefCell<HashMap<FileId, Input>>,
    parse: RefCell<HashMap<FileId, Memo<Document>>>,
    lower: RefCell<HashMap<FileId, Memo<Module>>>,
    naming: RefCell<HashMap<FileId, Memo<DocumentNamingComputation>>>,
    // One frame per in-flight body; a fetch pushes its key onto the top frame. Empty stack = a read made
    // outside any body (validation, or a top-level fetch), which records nothing onto a parent.
    stack: RefCell<Vec<Vec<QueryKey>>>,
    exec_counts: RefCell<ExecCounts>,
    parser: RefCell<Parser>,
    // A single shared, append-only interner (held inside the reused `LoweringContext`) gives every
    // occurrence of an identifier a stable `Symbol` id across recomputes, so memoized HIR compares equal
    // to a fresh recompute by value. `lower` resets the arena each call but keeps the interner.
    lowering: RefCell<LoweringContext>,
}

struct Input {
    text: Rc<String>,
    kind: DocumentKind,
    text_changed_at: Revision,
    kind_changed_at: Revision,
}

struct Memo<V> {
    value: Rc<V>,
    verified_at: Revision,
    changed_at: Revision,
    deps: Vec<QueryKey>,
}

// Value equality used for early-cutoff. The HIR `Module` and naming output are `Eq`, so cutoff is exact.
// `Document` is not `Eq`, but its tree is a pure function of the source, so source equality (cheap rope
// compare) is a sound proxy: equal source => equal parse for everything downstream reads.
trait CutoffEq {
    fn cutoff_eq(&self, other: &Self) -> bool;
}

impl CutoffEq for Document {
    fn cutoff_eq(&self, other: &Self) -> bool {
        self.rope() == other.rope()
    }
}

impl CutoffEq for Module {
    fn cutoff_eq(&self, other: &Self) -> bool {
        self == other
    }
}

impl CutoffEq for DocumentNamingComputation {
    fn cutoff_eq(&self, other: &Self) -> bool {
        self == other
    }
}

impl Engine {
    pub fn new() -> Self {
        Engine {
            revision: Cell::new(0),
            inputs: RefCell::new(HashMap::new()),
            parse: RefCell::new(HashMap::new()),
            lower: RefCell::new(HashMap::new()),
            naming: RefCell::new(HashMap::new()),
            stack: RefCell::new(Vec::new()),
            exec_counts: RefCell::new(ExecCounts::default()),
            parser: RefCell::new(new_parser().expect("spike: R grammar should load")),
            lowering: RefCell::new(LoweringContext::new()),
        }
    }

    // INPUT query: bump the revision and record this input's changed-at. The revision bumps on every set
    // (it is the engine's logical clock), but `changed_at` is *backdated* when the new (text, kind) equals
    // the old — salsa's input value-eq / backdating. This is what makes a no-op re-set cut off at the
    // input layer: dependents see an unchanged `changed_at` and stay green without running a single body.
    pub fn set_input(&mut self, file: FileId, path: &str, text: String) {
        let revision = self.revision.get() + 1;
        self.revision.set(revision);
        let kind = if path.contains("/R/") {
            DocumentKind::Package
        } else {
            DocumentKind::Script
        };
        let mut inputs = self.inputs.borrow_mut();
        let (text_changed_at, kind_changed_at) = match inputs.get(&file) {
            Some(previous) => (
                if *previous.text == text {
                    previous.text_changed_at
                } else {
                    revision
                },
                if previous.kind == kind {
                    previous.kind_changed_at
                } else {
                    revision
                },
            ),
            None => (revision, revision),
        };
        inputs.insert(
            file,
            Input {
                text: Rc::new(text),
                kind,
                text_changed_at,
                kind_changed_at,
            },
        );
    }

    pub fn remove_input(&mut self, file: FileId) {
        let revision = self.revision.get() + 1;
        self.revision.set(revision);
        self.inputs.borrow_mut().remove(&file);
        self.parse.borrow_mut().remove(&file);
        self.lower.borrow_mut().remove(&file);
        self.naming.borrow_mut().remove(&file);
    }

    pub fn revision(&self) -> Revision {
        self.revision.get()
    }

    pub fn exec_counts(&self) -> ExecCounts {
        *self.exec_counts.borrow()
    }

    pub fn memo_entry_count(&self) -> usize {
        self.parse.borrow().len() + self.lower.borrow().len() + self.naming.borrow().len()
    }

    // Rough retained-size estimate of the memo tables: the fixed per-entry memo header for all three
    // tables plus the recorded dependency edges. It deliberately excludes the deep `value` payloads
    // (trees, HIR arenas) — those are the unavoidable analysis outputs, not framework overhead; this
    // isolates the bookkeeping the engine adds on top.
    pub fn estimated_memo_overhead_bytes(&self) -> usize {
        let header = std::mem::size_of::<Memo<()>>();
        let edge = std::mem::size_of::<QueryKey>();
        let dep_edges: usize = self.parse.borrow().values().map(|m| m.deps.len()).sum::<usize>()
            + self.lower.borrow().values().map(|m| m.deps.len()).sum::<usize>()
            + self.naming.borrow().values().map(|m| m.deps.len()).sum::<usize>();
        self.memo_entry_count() * header + dep_edges * edge
    }

    pub fn fetch_parse(&self, file: FileId) -> Rc<Document> {
        self.record_dep(QueryKey {
            kind: QueryKind::Parse,
            file,
        });
        self.verify_derived(file, &self.parse, parse_body, |c| &mut c.parse);
        self.parse
            .borrow()
            .get(&file)
            .map(|memo| Rc::clone(&memo.value))
            .expect("spike: parse memo present after verify")
    }

    pub fn fetch_lower(&self, file: FileId) -> Rc<Module> {
        self.record_dep(QueryKey {
            kind: QueryKind::Lower,
            file,
        });
        self.verify_derived(file, &self.lower, lower_body, |c| &mut c.lower);
        self.lower
            .borrow()
            .get(&file)
            .map(|memo| Rc::clone(&memo.value))
            .expect("spike: lower memo present after verify")
    }

    pub fn fetch_local_naming(&self, file: FileId) -> Rc<DocumentNamingComputation> {
        self.record_dep(QueryKey {
            kind: QueryKind::LocalNaming,
            file,
        });
        self.verify_derived(file, &self.naming, naming_body, |c| &mut c.naming);
        self.naming
            .borrow()
            .get(&file)
            .map(|memo| Rc::clone(&memo.value))
            .expect("spike: naming memo present after verify")
    }

    // The drift oracle: recompute the whole chain from the current input WITHOUT consulting or touching
    // the memo tables, using the engine's own parser + interner so the result is value-comparable to the
    // memoized fetch. This is the framework analog of "incremental == full rebuild". It must not record
    // dependencies or bump exec counts, so it calls the production functions directly rather than the
    // query bodies.
    pub fn recompute_chain(&self, file: FileId) -> (String, Module, DocumentNamingComputation) {
        let (text, kind) = {
            let inputs = self.inputs.borrow();
            let input = inputs.get(&file).expect("spike: input set for recompute");
            (Rc::clone(&input.text), input.kind)
        };
        let document = {
            let mut parser = self.parser.borrow_mut();
            Document::parse(&mut parser, &text).expect("spike: source should parse")
        };
        let sexp = document.tree().root_node().to_sexp();
        let mut lowering = self.lowering.borrow_mut();
        let module = lower(&document, &mut lowering);
        let naming = resolve_document_locally(DocumentId(file), &module, lowering.interner(), kind);
        (sexp, module, naming)
    }

    pub fn parse_sexp(&self, file: FileId) -> String {
        self.fetch_parse(file).tree().root_node().to_sexp()
    }

    fn record_dep(&self, key: QueryKey) {
        if let Some(frame) = self.stack.borrow_mut().last_mut() {
            frame.push(key);
        }
    }

    // The generic red-green core: validate-or-recompute one derived query and return its `changed_at`.
    // The three queries differ only in `(table, body, counter)`, so this is the single place the
    // algorithm lives; the bodies are plain registered closures.
    fn verify_derived<V: CutoffEq>(
        &self,
        file: FileId,
        table: &RefCell<HashMap<FileId, Memo<V>>>,
        body: fn(&Self, FileId) -> V,
        counter: fn(&mut ExecCounts) -> &mut u64,
    ) -> Revision {
        let revision = self.revision.get();
        let cached = table
            .borrow()
            .get(&file)
            .map(|memo| (memo.verified_at, memo.changed_at, memo.deps.clone()));

        if let Some((verified_at, changed_at, deps)) = cached {
            if verified_at == revision {
                return changed_at; // green
            }
            // Deep-validate recorded deps. Validation does not record onto any frame (we are not inside a
            // body), and never re-borrows `table` (a query's deps are always a *different* table/kind).
            let max_dep_changed = deps
                .iter()
                .map(|dep| self.changed_at_of(*dep))
                .max()
                .unwrap_or(0);
            if max_dep_changed <= verified_at {
                // Early cutoff: nothing we read has changed since we were verified.
                if let Some(memo) = table.borrow_mut().get_mut(&file) {
                    memo.verified_at = revision;
                }
                return changed_at;
            }
        }

        *counter(&mut self.exec_counts.borrow_mut()) += 1;
        self.stack.borrow_mut().push(Vec::new());
        let value = body(self, file);
        let deps = self.stack.borrow_mut().pop().unwrap_or_default();

        let mut table = table.borrow_mut();
        // Cutoff propagation: if the recomputed value equals the old one, keep the old `changed_at` so
        // consumers can early-cut; otherwise this query changed at the current revision.
        let changed_at = match table.get(&file) {
            Some(old) if old.value.cutoff_eq(&value) => old.changed_at,
            _ => revision,
        };
        table.insert(
            file,
            Memo {
                value: Rc::new(value),
                verified_at: revision,
                changed_at,
                deps,
            },
        );
        changed_at
    }

    fn changed_at_of(&self, key: QueryKey) -> Revision {
        match key.kind {
            QueryKind::SourceText => self
                .inputs
                .borrow()
                .get(&key.file)
                .map(|input| input.text_changed_at)
                .unwrap_or(0),
            QueryKind::DocumentKind => self
                .inputs
                .borrow()
                .get(&key.file)
                .map(|input| input.kind_changed_at)
                .unwrap_or(0),
            QueryKind::Parse => {
                self.verify_derived(key.file, &self.parse, parse_body, |c| &mut c.parse)
            }
            QueryKind::Lower => {
                self.verify_derived(key.file, &self.lower, lower_body, |c| &mut c.lower)
            }
            QueryKind::LocalNaming => {
                self.verify_derived(key.file, &self.naming, naming_body, |c| &mut c.naming)
            }
        }
    }

    fn fetch_input_text(&self, file: FileId) -> Rc<String> {
        self.record_dep(QueryKey {
            kind: QueryKind::SourceText,
            file,
        });
        self.inputs
            .borrow()
            .get(&file)
            .map(|input| Rc::clone(&input.text))
            .expect("spike: input text set")
    }

    // The naming body reads the document *kind*, bypassing parse/lower. Two hazards converge here. First
    // the "untracked read": the ergonomically tempting version borrows `self.inputs` without recording,
    // so a same-text package<->script flip would never re-run naming on the stale kind — fixed by
    // recording the dep. Second, granularity: the kind must be its *own* input query, not bundled with
    // the source text, or naming would depend on the whole text and re-run on every text-only edit
    // (defeating the lower->naming cutoff). Hence `DocumentKind` is a separate fine-grained input.
    fn fetch_input_kind(&self, file: FileId) -> DocumentKind {
        self.record_dep(QueryKey {
            kind: QueryKind::DocumentKind,
            file,
        });
        self.inputs
            .borrow()
            .get(&file)
            .map(|input| input.kind)
            .unwrap_or(DocumentKind::Package)
    }
}

fn parse_body(engine: &Engine, file: FileId) -> Document {
    let text = engine.fetch_input_text(file);
    let mut parser = engine.parser.borrow_mut();
    Document::parse(&mut parser, &text).expect("spike: source should parse")
}

fn lower_body(engine: &Engine, file: FileId) -> Module {
    let document = engine.fetch_parse(file);
    let mut lowering = engine.lowering.borrow_mut();
    lower(&document, &mut lowering)
}

fn naming_body(engine: &Engine, file: FileId) -> DocumentNamingComputation {
    let module = engine.fetch_lower(file);
    let kind = engine.fetch_input_kind(file);
    let lowering = engine.lowering.borrow();
    resolve_document_locally(DocumentId(file), &module, lowering.interner(), kind)
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}
