//! Class-1 IDE granularity (cutover constraint 2): a per-keystroke point query (hover/inlay/signature)
//! must be O(1)-on-cached-`Typecheck` — a point query on an **unchanged** file re-runs **zero** `Typecheck`
//! bodies. Proven directly with the engine's exec counters: the `EngineIde` priming for a Class-1 feature
//! fetches `Typecheck(target)` only, so a repeated query on a warm engine recomputes nothing, an unrelated
//! edit recomputes nothing for the untouched file (blast-radius), and only editing the target's own body
//! re-runs its `Typecheck` (exactly once — never stale).

use {
    analysis::{naming::DocumentKind, text::TextPosition},
    engine::{
        Engine,
        ide_view::{EngineIde, PathTable},
        queries::{Config, FileId, Key, RoughlyQueries},
    },
    std::path::PathBuf,
};

const BASE: &str = "/ws";

fn package_path(id: FileId) -> PathBuf {
    PathBuf::from(format!("{BASE}/R/f{id:04}.R"))
}

fn typing_config() -> Config {
    Config {
        typing: true,
        ..Config::default()
    }
}

// A long-lived engine plus its host `PathTable`, mutated by `edit` and queried through a fresh `EngineIde`
// per call (the view holds only borrows, so it is created and dropped around each `&mut` edit).
struct Host {
    engine: Engine<RoughlyQueries>,
    paths: PathTable,
}

impl Host {
    fn new(files: &[(FileId, &str)]) -> Host {
        let mut engine = Engine::new(RoughlyQueries::new());
        let mut paths = PathTable::new(PathBuf::from(BASE));
        for (id, source) in files {
            engine.set_input(Key::SourceText(*id), (*source).to_owned());
            engine.set_input(Key::DocumentKind(*id), DocumentKind::Package);
            paths.insert(*id, package_path(*id));
        }
        engine.set_input(
            Key::ProjectFiles,
            files.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        );
        engine.set_input(Key::Config, typing_config());
        Host { engine, paths }
    }

    fn edit(&mut self, id: FileId, source: &str) {
        self.engine
            .set_input(Key::SourceText(id), source.to_owned());
    }

    // Each feature drives the genuine `EngineIde` priming path; the result is discarded because the
    // exec counters (not the output) are what these tests observe.
    fn hover(&self, id: FileId, position: TextPosition) {
        let _ = EngineIde::new(&self.engine, &self.paths).hover(&package_path(id), position);
    }

    fn inlay_hints(&self, id: FileId) {
        let _ = EngineIde::new(&self.engine, &self.paths).inlay_hints(&package_path(id), None);
    }

    fn signature_help(&self, id: FileId, position: TextPosition) {
        let _ =
            EngineIde::new(&self.engine, &self.paths).signature_help(&package_path(id), position);
    }

    fn completion(&self, id: FileId, position: TextPosition) {
        let _ = EngineIde::new(&self.engine, &self.paths).completion(&package_path(id), position);
    }

    fn definition(&self, id: FileId, position: TextPosition) {
        let _ = EngineIde::new(&self.engine, &self.paths).definition(&package_path(id), position);
    }

    fn typecheck_runs(&self, id: FileId) -> u64 {
        self.engine.group().typecheck_runs(id)
    }

    fn package_symbol_index_runs(&self) -> u64 {
        self.engine.group().package_symbol_index_runs()
    }
}

const ORIGIN: TextPosition = TextPosition {
    line_index: 0,
    character_index: 0,
};

#[test]
fn repeated_point_query_reruns_no_typecheck() {
    // A referrer (file 0) of a typed global defined in file 1 — a real cross-file Class-1 query.
    let host = Host::new(&[
        (0, "result <- shared_fn(2L)"),
        (
            1,
            "#: fn(x: integer) -> integer\nshared_fn <- function(x) x + x",
        ),
    ]);

    host.hover(0, ORIGIN); // warm: runs Typecheck(0) once
    let baseline = host.typecheck_runs(0);
    assert_eq!(
        baseline, 1,
        "warming should run the target's Typecheck exactly once"
    );

    // The headline property: a point query on an unchanged file re-runs zero Typecheck bodies.
    for _ in 0..5 {
        host.hover(0, ORIGIN);
    }
    assert_eq!(
        host.typecheck_runs(0),
        baseline,
        "a repeated point query on an unchanged file must re-run zero Typecheck bodies",
    );
}

// Warm a feature, then repeat it on the unchanged workspace; assert the target's `Typecheck` body re-runs
// zero times. Holds for both the features that prime `Typecheck(target)` (hover/signature/inlay — cached
// after warming) and those that never prime it (definition/completion — never touch it).
fn assert_no_typecheck_rerun(host: &Host, name: &str, run: impl Fn(&Host)) {
    run(host); // warm
    let before = host.typecheck_runs(0);
    for _ in 0..3 {
        run(host);
    }
    assert_eq!(
        host.typecheck_runs(0),
        before,
        "{name} re-ran Typecheck on an unchanged file",
    );
}

#[test]
fn all_class1_features_rerun_no_typecheck_on_unchanged_file() {
    // A referrer (file 0) with a call (`shared_fn(2L)` — signature target), a local binding (`local_value`
    // — inlay target), and a cross-file global use (`shared_fn` — hover/definition/completion target),
    // defined in file 1.
    let host = Host::new(&[
        (0, "result <- shared_fn(2L)\nlocal_value <- 1L"),
        (
            1,
            "#: fn(x: integer) -> integer\nshared_fn <- function(x) x + x",
        ),
    ]);

    let on_use = TextPosition {
        line_index: 0,
        character_index: 12,
    }; // inside the `shared_fn` use
    let in_args = TextPosition {
        line_index: 0,
        character_index: 20,
    }; // inside the call arguments
    let after_local = TextPosition {
        line_index: 1,
        character_index: 11,
    }; // a completion context

    assert_no_typecheck_rerun(&host, "hover", |host| host.hover(0, on_use));
    assert_no_typecheck_rerun(&host, "signature_help", |host| {
        host.signature_help(0, in_args)
    });
    assert_no_typecheck_rerun(&host, "inlay_hints", |host| host.inlay_hints(0));
    assert_no_typecheck_rerun(&host, "definition", |host| host.definition(0, on_use));
    assert_no_typecheck_rerun(&host, "completion", |host| host.completion(0, after_local));

    // Completion additionally fetches PackageSymbolIndex (the lone all-files fold). A repeat on an unchanged
    // workspace must NOT re-fold it — the names-only cutoff keeps the point query off the O(package) path.
    host.completion(0, after_local);
    let index_before = host.package_symbol_index_runs();
    for _ in 0..3 {
        host.completion(0, after_local);
    }
    assert_eq!(
        host.package_symbol_index_runs(),
        index_before,
        "completion re-folded PackageSymbolIndex on an unchanged workspace",
    );
}

#[test]
fn unrelated_edit_then_point_query_reruns_no_typecheck() {
    // Two independent files: 0 references no global, so nothing 1 contributes can reach it.
    let mut host = Host::new(&[(0, "value <- 1L"), (1, "other <- 2L")]);

    host.hover(0, ORIGIN); // warm
    let baseline = host.typecheck_runs(0);
    assert_eq!(baseline, 1);

    // Edit the unrelated file, then query the untouched one: blast-radius means its Typecheck stays cached.
    host.edit(1, "other <- 3L");
    host.hover(0, ORIGIN);
    assert_eq!(
        host.typecheck_runs(0),
        baseline,
        "an edit to an unrelated file must not re-run the queried file's Typecheck",
    );
}

#[test]
fn editing_target_body_reruns_its_typecheck_exactly_once() {
    // Nothing references file 0, so editing its body recomputes only its own Typecheck — and it *must*
    // recompute (the result is not stale), exactly once.
    let mut host = Host::new(&[(0, "value <- 1L")]);

    host.hover(0, ORIGIN); // warm
    let baseline = host.typecheck_runs(0);
    assert_eq!(baseline, 1);

    host.edit(0, "value <- 2L");
    host.hover(0, ORIGIN);
    assert_eq!(
        host.typecheck_runs(0),
        baseline + 1,
        "editing the target's own body must re-run its Typecheck exactly once (never stale)",
    );

    // And a second query after the edit, with no further change, re-runs nothing again.
    host.hover(0, ORIGIN);
    assert_eq!(host.typecheck_runs(0), baseline + 1);
}
