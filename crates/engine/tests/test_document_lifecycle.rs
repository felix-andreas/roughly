//! LSP document-lifecycle simulation against the real incremental engine.
//!
//! # Why this exists
//!
//! The differential net drives the engine through *state* edits (set every current file's inputs, then
//! compare against a from-scratch rebuild). It does not drive the engine through the *events* an editor
//! actually sends — `did_open`, `did_change`, `did_save`, `did_close`, `did_change_watched_files` — and
//! those events do not map one-to-one onto engine inputs: a `did_change` is an incremental range edit
//! against an open buffer, a `did_close` of an on-disk package file reverts the engine to disk text, a
//! watched change to an *open* file is ignored (the buffer is authoritative), and a watched
//! create/delete adds or tombstones a `SourceText`. This harness reproduces `EngineWorker`'s
//! event → engine-input mapping (`crates/roughly/src/server.rs`) so those incremental paths are exercised
//! and the load-bearing invariants that had no fixture can be pinned as scripted event sequences:
//!
//! - a transiently-malformed `did_change` emits no spurious semantic diagnostics (the `Lower` empty-module
//!   short-circuit), and a following well-formed edit restores them — the invariant `test_malformed_lower`
//!   pins in isolation, now proven through the real edit path;
//! - a watched-file add/delete changes cross-file resolution, exactly as an external file appearing or
//!   disappearing on disk would.
//!
//! Cancellation ("latest edit wins") is a separate concern and is pinned by `test_cancellation` against the
//! same `fetch_cancellable` / `with_cancellation` primitives the worker uses; it is not re-driven here.
//!
//! A focused Rust harness is used rather than an extended fixture DSL: the behaviors above are
//! event-kind-specific and depend on a live open-buffer-vs-disk duality that the declarative `MultiFile`
//! fixture shape (whole-file create / range edit / delete / move, with no open-set or disk model) cannot
//! express without inventing several new operations and a buffer model. The engine's incremental tests are
//! already Rust drivers (`test_differential`), and this joins that family, reusing the exact server mapping.

use {
    analysis::{
        Diagnostic, DiagnosticCode,
        document::{Document, DocumentChange},
        naming::DocumentKind,
        text::{TextPosition, TextRange},
        tree::new_parser,
    },
    engine::{
        Engine,
        queries::{Config, FileDiagnostics, FileId, Key, RoughlyQueries, parse_source_input},
    },
    std::collections::BTreeMap,
    tree_sitter::{Parser, Range},
};

const BASE: &str = "/ws";

// The workspace root is `/ws`; package files live under `/ws/R/` (what production keys `is_package_path`
// on) and scripts elsewhere, so the harness classifies exactly as the server does.
fn is_package_path(path: &str) -> bool {
    path.starts_with(&format!("{BASE}/R/"))
}

// The `ProjectFiles` order production feeds the engine: package files first, then scripts, each ascending
// by workspace-relative path — the order the server's `rebuild_project_files` produces so the engine's
// last-writer-wins symbol index picks the same winner as the from-scratch oracle.
fn project_files_order(paths: &BTreeMap<String, FileId>) -> Vec<FileId> {
    let mut entries = paths
        .iter()
        .map(|(path, file)| {
            let key = path.strip_prefix(&format!("{BASE}/")).unwrap_or(path);
            (!is_package_path(path), key.to_owned(), *file)
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries.into_iter().map(|(_, _, file)| file).collect()
}

// A driver that owns the engine and, like the LSP server, keeps three separate views: the engine's inputs,
// the set of open editor buffers (each a live `Document` so `did_change` applies an incremental range edit
// exactly as the server does), and an in-memory model of on-disk text (so `did_close` can revert to disk
// and a watched create/change/delete can be simulated). Each `did_*` method reproduces the corresponding
// `EngineWorker` handler's engine-input mutations.
struct Lifecycle {
    engine: Engine<RoughlyQueries>,
    parser: Parser,
    config: Config,
    file_ids: BTreeMap<String, FileId>,
    next_file_id: FileId,
    open_buffers: BTreeMap<String, Document>,
    disk: BTreeMap<String, String>,
}

impl Lifecycle {
    fn new(config: Config) -> Lifecycle {
        let mut lifecycle = Lifecycle {
            engine: Engine::new(RoughlyQueries::new()),
            parser: new_parser().expect("harness: R grammar should load"),
            config,
            file_ids: BTreeMap::new(),
            next_file_id: 0,
            open_buffers: BTreeMap::new(),
            disk: BTreeMap::new(),
        };
        lifecycle
            .engine
            .set_input(Key::Config, lifecycle.config.clone());
        lifecycle
    }

    // Seed a file's on-disk text before the session starts (the initial workspace scan the server does in
    // `initialize`). Package files discovered on disk are set as engine inputs immediately.
    fn seed_disk_file(&mut self, path: &str, text: &str) {
        self.disk.insert(path.to_owned(), text.to_owned());
        if is_package_path(path) {
            self.set_source_input(path, text);
        }
        self.rebuild_project_files();
    }

    fn file_id_for(&mut self, path: &str) -> FileId {
        if let Some(id) = self.file_ids.get(path) {
            return *id;
        }
        let id = self.next_file_id;
        self.next_file_id += 1;
        self.file_ids.insert(path.to_owned(), id);
        id
    }

    // Mirror of `EngineWorker::set_source_input`: set the file's text and kind inputs, allocating a stable
    // id on first sight. Does not touch `ProjectFiles`.
    fn set_source_input(&mut self, path: &str, text: &str) {
        let file = self.file_id_for(path);
        self.engine
            .set_input(Key::SourceText(file), parse_source_input(text));
        self.engine.set_input(
            Key::DocumentKind(file),
            if is_package_path(path) {
                DocumentKind::Package
            } else {
                DocumentKind::Script
            },
        );
    }

    // Mirror of `EngineWorker::retract_source_input`: tombstone the file's inputs and drop its id.
    fn retract_source_input(&mut self, path: &str) {
        if let Some(file) = self.file_ids.remove(path) {
            self.engine.remove_input(&Key::SourceText(file));
            self.engine.remove_input(&Key::DocumentKind(file));
        }
    }

    fn rebuild_project_files(&mut self) {
        let order = project_files_order(&self.file_ids);
        self.engine.set_input(Key::ProjectFiles, order);
    }

    // `did_open`: the editor opens a buffer with the given text; the buffer becomes authoritative, the file
    // joins the set, and its text/kind inputs are set.
    fn did_open(&mut self, path: &str, text: &str) {
        let document = Document::parse(&mut self.parser, text)
            .expect("harness: open document buffer should parse");
        self.open_buffers.insert(path.to_owned(), document);
        self.set_source_input(path, text);
        self.rebuild_project_files();
    }

    // `did_change`: one incremental range edit against the open buffer (byte-offset columns, tree-sitter
    // `Point` semantics — the same conversion the server performs), then the whole new buffer text is
    // re-set as `SourceText`. The file set is unchanged, so no `ProjectFiles` rebuild.
    fn did_change(&mut self, path: &str, range: TextRange, replacement: &str) {
        let buffer = self
            .open_buffers
            .get_mut(path)
            .expect("harness: did_change on a non-open document");
        buffer
            .edit(
                &mut self.parser,
                std::slice::from_ref(&DocumentChange {
                    range,
                    text: replacement.to_owned(),
                }),
            )
            .expect("harness: incremental edit range should be valid");
        let text = buffer.rope().to_string();
        self.set_source_input(path, &text);
    }

    // `did_save`: the buffer's current text is written to disk. The engine already reflects the synced
    // buffer text (every `did_change` set it), so saving mutates no engine input — exactly the server's
    // behavior. Modeled so a later `did_close` reverts to the saved text, not the pre-edit disk text.
    fn did_save(&mut self, path: &str) {
        let text = self
            .open_buffers
            .get(path)
            .expect("harness: did_save on a non-open document")
            .rope()
            .to_string();
        self.disk.insert(path.to_owned(), text);
    }

    // `did_close`: the buffer is dropped. A package file still on disk reverts the engine to its on-disk
    // text (discarding unsaved edits); the file set is unchanged. Otherwise the file is retracted and the
    // set shrinks.
    fn did_close(&mut self, path: &str) {
        self.open_buffers.remove(path);
        if is_package_path(path) && self.disk.contains_key(path) {
            let text = self.disk[path].clone();
            self.set_source_input(path, &text);
        } else {
            self.retract_source_input(path);
            self.rebuild_project_files();
        }
    }

    // `did_change_watched_files` create/change: an external tool wrote `path` on disk. A change to an *open*
    // file is ignored (the buffer is authoritative). A package file that is not open is (re)synced from
    // disk and the set is rebuilt.
    fn watched_write(&mut self, path: &str, text: &str) {
        self.disk.insert(path.to_owned(), text.to_owned());
        if is_package_path(path) && !self.open_buffers.contains_key(path) {
            self.set_source_input(path, text);
        }
        self.rebuild_project_files();
    }

    // `did_change_watched_files` delete: an external tool removed `path`. A non-open package file is
    // retracted and the set shrinks; a delete of an open file is ignored here (the buffer is authoritative).
    fn watched_delete(&mut self, path: &str) {
        self.disk.remove(path);
        if is_package_path(path) && !self.open_buffers.contains_key(path) {
            self.retract_source_input(path);
        }
        self.rebuild_project_files();
    }

    // The engine's current diagnostics for a file, rendered and config-gated exactly as production's
    // `document_diagnostics` (the same shape as the differential harness's `engine_diagnostics`).
    fn diagnostics(&self, path: &str) -> Vec<Diagnostic> {
        let file = self.file_ids[path];
        let file_diagnostics = self.engine.fetch::<FileDiagnostics>(Key::Diagnostics(file));
        let fallback = *self.engine.fetch::<Range>(Key::FallbackRange);
        let mut rendered = Vec::new();
        rendered.extend(file_diagnostics.naming.iter().cloned());
        rendered.extend(file_diagnostics.package_naming.iter().cloned());
        rendered.extend(file_diagnostics.lowering.iter().cloned());
        rendered.extend(file_diagnostics.lint.iter().cloned());
        if self.config.check.unused {
            rendered.extend(file_diagnostics.unused.iter().cloned());
        }
        if self.config.check.typing {
            self.engine.group().with_interner(|interner| {
                for error in &file_diagnostics.type_errors {
                    rendered.push(Diagnostic::from_inference_error(error, fallback, interner));
                }
            });
        }
        if self.config.check.strict {
            rendered.extend(file_diagnostics.strict_diagnostics.iter().cloned());
        }
        rendered
    }
}

fn typing_config() -> Config {
    Config {
        check: analysis::CheckConfig {
            typing: true,
            strict: false,
            unused: false,
        },
        lint: Default::default(),
    }
}

// A whole-line range `[line, 0)..(line, len)` (0-based line, byte columns) for `did_change` edits.
fn line_range(line: usize, start_column: usize, end_column: usize) -> TextRange {
    TextRange {
        start: TextPosition {
            line_index: line,
            character_index: start_column,
        },
        end: TextPosition {
            line_index: line,
            character_index: end_column,
        },
    }
}

fn has_code(diagnostics: &[Diagnostic], code: DiagnosticCode) -> bool {
    diagnostics.iter().any(|diagnostic| diagnostic.code == code)
}

// A transiently-malformed `did_change` emits no spurious semantic diagnostics, and a following well-formed
// edit restores them — the `Lower` empty-module short-circuit, driven through the real open/change path
// rather than a direct `set_input`.
#[test]
fn malformed_edit_suppresses_then_restores_diagnostics() {
    const PATH: &str = "/ws/R/main.R";
    let mut lifecycle = Lifecycle::new(typing_config());

    // Open a well-formed file that references an undefined name: a package-naming diagnostic is present.
    lifecycle.did_open(PATH, "y <- undefined_fn(1L)\n");
    let opened = lifecycle.diagnostics(PATH);
    assert!(
        has_code(&opened, DiagnosticCode::Unresolved),
        "the opened well-formed file resolves the reference and reports it unresolved: {opened:#?}"
    );

    // A transiently-broken keystroke: delete the closing paren, leaving `y <- undefined_fn(1L`. The tree now
    // carries an error node, so the file lowers to an empty module and every semantic class is empty.
    lifecycle.did_change(PATH, line_range(0, 20, 21), "");
    let broken = lifecycle.diagnostics(PATH);
    assert!(
        !has_code(&broken, DiagnosticCode::Unresolved)
            && !has_code(&broken, DiagnosticCode::TypeError)
            && !has_code(&broken, DiagnosticCode::Strict),
        "a malformed edit lowers to an empty module and emits no semantic diagnostics: {broken:#?}"
    );

    // Type the paren back: the file is well-formed again and the naming diagnostic returns.
    lifecycle.did_change(PATH, line_range(0, 20, 20), ")");
    let restored = lifecycle.diagnostics(PATH);
    assert!(
        has_code(&restored, DiagnosticCode::Unresolved),
        "restoring the well-formed source restores the diagnostic: {restored:#?}"
    );
}

// A watched-file create makes a name resolvable across files, and a watched-file delete makes it unresolved
// again — the engine reacting to an external file appearing and disappearing on disk (the
// `did_change_watched_files` add/tombstone path plus the `ProjectFiles` rebuild).
#[test]
fn watched_file_add_then_delete_changes_cross_file_resolution() {
    const USER: &str = "/ws/R/use.R";
    const DEF: &str = "/ws/R/def.R";
    let mut lifecycle = Lifecycle::new(typing_config());

    // Open a file that references `helper`, which no file defines yet: unresolved across the package.
    lifecycle.did_open(USER, "x <- helper(1L)\n");
    let before = lifecycle.diagnostics(USER);
    assert!(
        has_code(&before, DiagnosticCode::Unresolved),
        "with no definer, the cross-file reference is unresolved: {before:#?}"
    );

    // An external tool writes a file that defines `helper`. The watcher syncs it; the reference now resolves
    // package-globally and the naming diagnostic clears.
    lifecycle.watched_write(DEF, "helper <- function(n) n\n");
    let after_add = lifecycle.diagnostics(USER);
    assert!(
        !has_code(&after_add, DiagnosticCode::Unresolved),
        "the added definer resolves the cross-file reference: {after_add:#?}"
    );

    // The external tool deletes the definer again. The watcher tombstones it and the reference is unresolved
    // once more — the tombstone-and-revalidate path.
    lifecycle.watched_delete(DEF);
    let after_delete = lifecycle.diagnostics(USER);
    assert!(
        has_code(&after_delete, DiagnosticCode::Unresolved),
        "deleting the definer makes the reference unresolved again: {after_delete:#?}"
    );
}

// `did_close` of an on-disk package file with unsaved buffer edits reverts the engine to the on-disk text
// (the edits are discarded), while a save first persists the buffer so a later close keeps it.
#[test]
fn close_reverts_unsaved_edits_but_keeps_saved_edits() {
    const PATH: &str = "/ws/R/lib.R";
    let mut lifecycle = Lifecycle::new(typing_config());
    lifecycle.seed_disk_file(PATH, "good <- 1L\n");

    // Open the on-disk file and add a line that references an undefined name (an unsaved edit).
    lifecycle.did_open(PATH, "good <- 1L\n");
    lifecycle.did_change(PATH, line_range(1, 0, 0), "y <- missing(2L)\n");
    let edited = lifecycle.diagnostics(PATH);
    assert!(
        has_code(&edited, DiagnosticCode::Unresolved),
        "the unsaved edit introduces an unresolved reference: {edited:#?}"
    );

    // Close without saving: the engine reverts to the on-disk text, so the diagnostic is gone.
    lifecycle.did_close(PATH);
    let after_close = lifecycle.diagnostics(PATH);
    assert!(
        !has_code(&after_close, DiagnosticCode::Unresolved),
        "closing an unsaved package file reverts to disk and drops the edit's diagnostic: {after_close:#?}"
    );

    // Reopen, reintroduce the bad edit, then SAVE it to disk. Now a close preserves the diagnostic because
    // the engine's disk text carries the edit.
    lifecycle.did_open(PATH, "good <- 1L\n");
    lifecycle.did_change(PATH, line_range(1, 0, 0), "y <- missing(2L)\n");
    lifecycle.did_save(PATH);
    lifecycle.did_close(PATH);
    let after_save_close = lifecycle.diagnostics(PATH);
    assert!(
        has_code(&after_save_close, DiagnosticCode::Unresolved),
        "a saved edit survives close because disk now carries it: {after_save_close:#?}"
    );
}

// A watched-file change to an *open* document is ignored: the open buffer is authoritative, so the engine
// keeps the buffer text even when disk changes underneath.
#[test]
fn watched_change_to_open_file_is_ignored() {
    const PATH: &str = "/ws/R/open.R";
    let mut lifecycle = Lifecycle::new(typing_config());
    lifecycle.seed_disk_file(PATH, "a <- 1L\n");

    // Open the file and edit the buffer to a clean state.
    lifecycle.did_open(PATH, "a <- 1L\n");
    assert!(
        lifecycle.diagnostics(PATH).is_empty(),
        "the open buffer is clean"
    );

    // An external tool rewrites the on-disk file to something with an unresolved reference. Because the file
    // is open, the watcher ignores it and the engine keeps the clean buffer text.
    lifecycle.watched_write(PATH, "b <- missing(1L)\n");
    assert!(
        lifecycle.diagnostics(PATH).is_empty(),
        "a watched change to an open file is ignored; the buffer stays authoritative: {:#?}",
        lifecycle.diagnostics(PATH)
    );
}
