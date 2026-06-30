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
        self.engine.set_input(Key::SourceText(id), source.to_owned());
    }

    fn hover(&self, id: FileId, position: TextPosition) {
        // The result itself is irrelevant here — the priming (which fetches `Typecheck(id)`) is what the
        // counters observe — but hover is the genuine Class-1 surface, so this drives the real path.
        let _ = EngineIde::new(&self.engine, &self.paths).hover(&package_path(id), position);
    }

    fn typecheck_runs(&self, id: FileId) -> u64 {
        self.engine.group().typecheck_runs(id)
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
        (1, "#: fn(x: integer) -> integer\nshared_fn <- function(x) x + x"),
    ]);

    host.hover(0, ORIGIN); // warm: runs Typecheck(0) once
    let baseline = host.typecheck_runs(0);
    assert_eq!(baseline, 1, "warming should run the target's Typecheck exactly once");

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
