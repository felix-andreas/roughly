//! The LSP worker makes each IDE read cancellable by running it through `Engine::with_cancellation`
//! around an `EngineIde` query (see `EngineWorker::cancellable` in `crates/roughly/src/server.rs`). This
//! pins that exact pattern on the REAL `RoughlyQueries` + `EngineIde`: a read whose token is observed set
//! abandons (the engine unwinds `Cancelled` at its first recompute check, which sits at the start of
//! every recompute), and the same read with the token clear computes the real result. Together with the
//! generic `with_cancellation` concurrency test in `test_cancellation.rs` and the functional worker
//! coverage in `roughly`'s `test_lsp`, this closes gate (d)'s "reads are cancellable" requirement.

use {
    analysis::{LintConfig, TextPosition, naming::DocumentKind},
    engine::{
        Cancelled, Engine,
        ide_view::{EngineIde, PathTable},
        queries::{Config, FileId, Key, RoughlyQueries},
    },
    std::{
        path::PathBuf,
        sync::{Arc, atomic::AtomicBool},
    },
};

// Cancellation raises a `Cancelled` panic that runs the process hook before `with_cancellation` catches
// it, so suppress that payload (routine, not a fault) while leaving every other panic to the default hook.
fn silence_cancelled_panics() {
    let previous = Arc::new(std::panic::take_hook());
    std::panic::set_hook(Box::new(move |info| {
        if !info.payload().is::<Cancelled>() {
            previous(info);
        }
    }));
}

fn build_single_file_engine() -> (Engine<RoughlyQueries>, PathTable, PathBuf) {
    let base = PathBuf::from("/pkg");
    let path = base.join("R/a.R");
    let file: FileId = 0;

    let mut engine = Engine::new(RoughlyQueries::new());
    engine.set_input(
        Key::SourceText(file),
        "add <- function(x, y) x + y\nz <- add(1L, 2L)\n".to_owned(),
    );
    engine.set_input(Key::DocumentKind(file), DocumentKind::Package);
    engine.set_input(Key::ProjectFiles, vec![file]);
    engine.set_input(
        Key::Config,
        Config {
            typing: true,
            strict: false,
            unused: false,
            lint: LintConfig::default(),
        },
    );

    let mut paths = PathTable::new(base);
    paths.insert(file, path.clone());
    (engine, paths, path)
}

#[test]
fn ide_read_abandons_under_a_set_token_and_completes_when_clear() {
    silence_cancelled_panics();
    let (engine, paths, path) = build_single_file_engine();
    // The binding `add` on line 0 — a top-level definition that has an inferred type to hover.
    let position = TextPosition {
        line_index: 0,
        character_index: 0,
    };

    // Token observed set (an edit flipped it): the cold read abandons at its first recompute check, so
    // the worker would answer the type's empty default rather than block on superseded work.
    let cancelled = engine.with_cancellation(Arc::new(AtomicBool::new(true)), || {
        EngineIde::new(&engine, &paths).hover(&path, position)
    });
    assert_eq!(
        cancelled,
        Err(Cancelled),
        "an IDE read whose token is set abandons instead of producing a result"
    );

    // Token clear: the same read computes the real hover (proving the cancellation wrap did not corrupt
    // the read path — the engine is left consistent and the result is the genuine hover).
    let completed = engine
        .with_cancellation(Arc::new(AtomicBool::new(false)), || {
            EngineIde::new(&engine, &paths).hover(&path, position)
        })
        .expect("a read with a clear token is never cancelled");
    assert!(
        completed.is_some(),
        "the uncancelled read returns the real hover result for `add`"
    );
}
