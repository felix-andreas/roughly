// The re-export fixed-point (`DESIGN.md` §5). These drive the real `analysis` phase chain through the
// engine and exercise the SCC routing in `GlobalScheme`:
//
//   - a TRIVIAL SCC (acyclic chain): `GlobalScheme(a)` resolves an acyclic re-export
//     `a <- b <- … <- concrete` by plain fetch recursion (no round cap to truncate it);
//   - a NON-TRIVIAL SCC (a genuine cycle) routes `GlobalScheme` through `InterfaceScc(scc)`, a single
//     bounded fixed-point body that pins a pure mutual cycle to `Unknown` and converges.
//
// The re-export graph is functional (a global re-exports at most one other), so every non-trivial SCC is a
// pure cycle with no concrete base — hence `Unknown`. The "near-bound" case is acyclic: in the engine a
// chain resolves by fetch recursion, which has no bound at all, so it strictly preserves the property the
// production round-cap fix bought (a deep chain is never cut off short to a stale `Unknown`).

use {
    analysis::{
        interner::Symbol,
        naming::DocumentKind,
        types::{CoreType, TypeScheme},
    },
    engine::{
        Engine,
        queries::{Config, FileDiagnostics, FileId, Key, RoughlyQueries, parse_source_input},
    },
};

fn setup(sources: &[(FileId, &str)]) -> Engine<RoughlyQueries> {
    let mut engine = Engine::new(RoughlyQueries::new());
    let files: Vec<FileId> = sources.iter().map(|(file, _)| *file).collect();
    engine.set_input(Key::ProjectFiles, files);
    engine.set_input(Key::Config, Config::default());
    for (file, source) in sources {
        engine.set_input(Key::SourceText(*file), parse_source_input(source));
        engine.set_input(Key::DocumentKind(*file), DocumentKind::Package);
    }
    engine
}

fn global_scheme(engine: &Engine<RoughlyQueries>, symbol: Symbol) -> Option<TypeScheme> {
    (*engine.fetch::<Option<TypeScheme>>(Key::GlobalScheme(symbol))).clone()
}

fn body_of(scheme: &Option<TypeScheme>) -> CoreType {
    scheme
        .as_ref()
        .map(|scheme| scheme.body.clone())
        .unwrap_or(CoreType::Unknown)
}

// MONOTONE RE-EXPORT CHAIN: `a <- b <- c <- (concrete)` resolves every link to the concrete scheme, and
// editing the concrete base updates the whole chain. No panic; the chain is acyclic (every link is a
// trivial SCC), so it travels the plain fetch-recursion path.
#[test]
fn monotone_reexport_chain_converges_to_concrete() {
    let mut engine = setup(&[
        (0, "a <- b"),
        (1, "b <- c"),
        (2, "c <- function() 1"),
        (3, "z <- a()"),
    ]);
    let a = engine.group().intern("a");
    let b = engine.group().intern("b");
    let c = engine.group().intern("c");

    // Each link is a trivial (acyclic) SCC: no fixed-point body is involved at all.
    assert!(engine.fetch::<Vec<Symbol>>(Key::SymbolScc(a)).is_empty());
    assert!(engine.fetch::<Vec<Symbol>>(Key::SymbolScc(b)).is_empty());

    let scheme_a = global_scheme(&engine, a);
    let scheme_c = global_scheme(&engine, c);
    assert!(scheme_a.is_some(), "head of the chain resolves");
    assert_ne!(
        body_of(&scheme_a),
        CoreType::Unknown,
        "the chain resolves to the concrete scheme, not Unknown"
    );
    assert_eq!(
        scheme_a, scheme_c,
        "every link equals the concrete base's scheme"
    );

    // The consumer `z <- a()` typechecks against a concrete `() -> integer`: no cross-file errors.
    let diagnostics = engine.fetch::<FileDiagnostics>(Key::Diagnostics(3));
    assert!(
        diagnostics.type_errors.is_empty(),
        "consumer of the re-export chain has no type errors: {:?}",
        diagnostics.type_errors
    );

    // Edit the concrete base: integer -> string. The change must propagate up the whole chain.
    engine.set_input(
        Key::SourceText(2),
        parse_source_input("c <- function() \"two\""),
    );
    let scheme_a_after = global_scheme(&engine, a);
    let scheme_c_after = global_scheme(&engine, c);
    assert_ne!(
        scheme_a, scheme_a_after,
        "editing the base updates the chain head"
    );
    assert_eq!(
        scheme_a_after, scheme_c_after,
        "the chain still tracks the (now string) concrete base"
    );
    assert_ne!(body_of(&scheme_a_after), CoreType::Unknown);
}

// GENUINE PERIOD-2 CYCLE: `a <- b; b <- a` with no concrete base. Both members route through one shared
// `InterfaceScc` memo, the fixed-point pins them to `Unknown` and converges (it does NOT spin the
// round cap and does NOT trip the accidental-cycle guard). A consumer sees `Unknown`, not a crash.
#[test]
fn period_two_cycle_pins_to_unknown_and_converges() {
    let engine = setup(&[(0, "a <- b"), (1, "b <- a"), (2, "z <- a()")]);
    let a = engine.group().intern("a");
    let b = engine.group().intern("b");

    // Both members are in the same non-trivial SCC, reported as the identical canonical (sorted) list.
    let scc_a = (*engine.fetch::<Vec<Symbol>>(Key::SymbolScc(a))).clone();
    let scc_b = (*engine.fetch::<Vec<Symbol>>(Key::SymbolScc(b))).clone();
    assert_eq!(scc_a.len(), 2, "a and b form a 2-cycle");
    assert_eq!(
        scc_a, scc_b,
        "every member projects the same canonical SCC, sharing one memo"
    );

    // The pure mutual re-export cycle holds no information: both resolve to Unknown.
    assert_eq!(body_of(&global_scheme(&engine, a)), CoreType::Unknown);
    assert_eq!(body_of(&global_scheme(&engine, b)), CoreType::Unknown);

    // Convergence witness: the SCC fixed-point body ran exactly once for the shared member list (it did
    // not panic and did not spin) — every member projected from the one converged sub-table.
    assert_eq!(
        engine.group().interface_scc_runs(&scc_a),
        1,
        "the SCC interface is computed once and shared, proving convergence (no spin / no panic)"
    );

    // A consumer typechecking against the cyclic re-export sees Unknown, not a crash.
    let diagnostics = engine.fetch::<FileDiagnostics>(Key::Diagnostics(2));
    let _ = diagnostics; // reaching here at all proves the cycle did not panic the engine.
}

// FETCH-SPINE DEPTH: a mechanically generated re-export chain thousands of links long must resolve,
// not abort the process. Each link is one query level on the host stack (fetch → validate → recompute
// → body → fetch …), so a chain this deep overflows any fixed thread stack; `validate`'s stack-growth
// guard is what keeps it alive. The chain is then revalidated after an edit to the concrete base,
// covering the dependency-walk recursion in `validate` itself (the second deep spine).
#[test]
fn very_deep_chain_does_not_overflow_the_stack() {
    // Deep enough that an unguarded spine aborts on any fixed thread stack (verified: without the
    // `validate` guard this chain dies of stack overflow well before the tail), small enough to keep
    // the suite fast.
    const LINKS: FileId = 2048;
    let mut sources: Vec<(FileId, String)> = Vec::new();
    for index in 0..LINKS {
        let name = format!("d{index}");
        let body = if index + 1 < LINKS {
            format!("{name} <- d{}", index + 1)
        } else {
            format!("{name} <- function() 1")
        };
        sources.push((index, body));
    }
    let borrowed: Vec<(FileId, &str)> = sources
        .iter()
        .map(|(file, body)| (*file, body.as_str()))
        .collect();
    let mut engine = setup(&borrowed);

    let head = engine.group().intern("d0");
    let scheme_head = global_scheme(&engine, head);
    assert!(
        scheme_head.is_some(),
        "the {LINKS}-link chain head resolves"
    );
    assert_ne!(body_of(&scheme_head), CoreType::Unknown);

    // Edit the concrete base: the head's revalidation walks the whole chain depth again.
    engine.set_input(
        Key::SourceText(LINKS - 1),
        parse_source_input(&format!("d{} <- function() \"two\"", LINKS - 1)),
    );
    let scheme_head_after = global_scheme(&engine, head);
    assert_ne!(
        scheme_head, scheme_head_after,
        "the edit propagates through all {LINKS} links"
    );
    assert_ne!(body_of(&scheme_head_after), CoreType::Unknown);
}

// CHAIN NEAR THE BOUND: a re-export chain whose length exceeds any `#members + slack`-style cap still
// resolves fully to the concrete scheme. In the engine an acyclic chain travels fetch recursion, which has
// no round cap, so it can never be truncated to a stale `Unknown` — the regression the production
// round-cap fix closed, preserved here by construction.
#[test]
fn deep_chain_is_not_truncated() {
    const LINKS: FileId = 16; // far beyond a small fixed cap, and beyond `#globals + small slack`.
    let mut sources: Vec<(FileId, String)> = Vec::new();
    // s0 <- s1 <- s2 <- ... <- s{LINKS-1} <- (concrete) on the last file.
    for index in 0..LINKS {
        let name = format!("s{index}");
        let body = if index + 1 < LINKS {
            format!("{name} <- s{}", index + 1)
        } else {
            format!("{name} <- function() 1")
        };
        sources.push((index, body));
    }
    let borrowed: Vec<(FileId, &str)> = sources
        .iter()
        .map(|(file, body)| (*file, body.as_str()))
        .collect();
    let engine = setup(&borrowed);

    let head = engine.group().intern("s0");
    let base = engine.group().intern(&format!("s{}", LINKS - 1));

    // The whole chain is acyclic: the head is a trivial SCC.
    assert!(engine.fetch::<Vec<Symbol>>(Key::SymbolScc(head)).is_empty());

    let scheme_head = global_scheme(&engine, head);
    assert!(scheme_head.is_some(), "the deep chain head resolves");
    assert_ne!(
        body_of(&scheme_head),
        CoreType::Unknown,
        "a chain near/over the bound is NOT truncated to a stale Unknown"
    );
    assert_eq!(
        scheme_head,
        global_scheme(&engine, base),
        "the head resolves to the concrete base's scheme across all {LINKS} links"
    );
}
