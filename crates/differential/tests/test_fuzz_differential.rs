//! The fuzz arm of the cross-stack differential: seeded random programs run
//! through BOTH stacks and compared with the same class + range-containment
//! policy as the fixture and corpus arms. The fixture arm only ever compares
//! sources someone thought to write down, and the corpus arm only real-world
//! code that is mostly clean — this arm sweeps the space between, where a
//! diagnostic class one stack models and the other silently lacks shows up
//! as a divergence instead of going unnoticed.
//!
//! The generator is differential-specific on purpose: unlike the semantics
//! fuzz harness (never-panic, determinism, incremental equivalence), this
//! pool is biased toward programs that PRODUCE findings both stacks model —
//! unresolved reads, unused bindings, type mismatches against annotations,
//! and annotation block-structure errors. Strict mode stays off: strict
//! attribution on recursive shapes diverges by design (see the fixture arm's
//! `ACCEPTED_DIVERGENCES`), and generic filters cannot express those
//! case-shaped acceptances.
//!
//! Divergences accepted by the shared oracle-deficit filters (forward
//! captures, Unknown tolerance) are counted, not failed. An oracle panic
//! (the legacy fixed-point `debug_assert`s on growing self-references)
//! skips the input. Runs are deterministic per seed; `FUZZ_ITERS` scales
//! the budget.

use differential::{
    filter_legacy_slot_tolerance, filter_model_divergences, filter_oracle_deficits,
    filter_unknown_type_additions, findings_match, legacy_findings, new_findings, new_stack_facts,
};
use semantics::DocumentKind;
use std::collections::BTreeMap;
use std::fmt::Write as _;

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }

    fn chance(&mut self, numerator: u64, denominator: u64) -> bool {
        self.next() % denominator < numerator
    }
}

/// Small colliding pool (unresolved reads and shadowing arise constantly)
/// plus one multibyte name so range agreement is byte-exact, not
/// ASCII-lucky.
const NAMES: &[&str] = &["a", "b", "f", "g", "x", "helper", "größe"];

/// Item templates; `{n}` is the defined name, `{m}` an arbitrary (often
/// different, often undefined) name from the pool.
const TEMPLATES: &[&str] = &[
    "{n} <- {m}",
    "{n} <- {n} + 1L",
    "{n} <- {m} + 1L",
    "{n} <- 1L + \"s\"",
    "{n} <- function(p) {m}(p)",
    "{n} <- function(p) p + 1L",
    "{n} <- function() {n}()",
    "{n} <- c({m}, 1L)",
    "{n} <- list(x = 1, y = {m})",
    "{n} <- if ({m} > 1L) {m} else NULL",
    "{n} <- function(p) if (is.null(p)) 0L else p",
    "for (i in 1:3) {n} <- {m} + i",
    "{n} <- local({ t <- {m}; t })",
    "{n} <- \\(q) q + {m}",
    "print({m})",
    "{n} <- sum({m}, 1L)",
    "{n} <- {m}$field",
    "{n} <- {m}[[1L]]",
    "#: integer\n{n} <- 1L",
    "#: integer\n{n} <- \"s\"",
    "#: fn(x: integer) -> integer\n{n} <- function(x) x",
    "#: fn(x: integer) -> character\n{n} <- function(x) x",
    "#: fn(x: integer) -> integer\n{n} <- function(x) {m}(x)",
    "#: @alias Alias {integer}\n\n#: Alias\n{n} <- {m}",
    "#: @type Point {list{v: double}}\n\n#: @new Point\n{n} <- list(v = 1)",
    "#: @trust fn(x: integer) -> character\n{n} <- function(x) x",
    "#: @param x {integer}\n#: @return {integer}\n{n} <- function(x) x",
    "#: @type Bad {integer}\n#: integer\n{n} <- 1L",
    "#: integer\n#: character\n{n} <- 1L",
    "#: Instype\n{n} <- 1L",
    "#: @type Shape {list{part: Instype}}\n\n#: @new Shape\n{n} <- list(part = 1)",
    "#: fn(x: Instype) -> integer\n{n} <- function(x) x",
    "{n} <- with({m}, x + y)",
    "{n} <- stats::sd(c(1, 2))",
    "{n} <- nosuchpkg::mystery()",
    "{n} <- {m} |> print()",
    "{n} <- y ~ {m}",
    "while ({m} > 0L) {n} <- {n} - 1L",
    "{n} <- function(...) list(...)",
    "{n} <- switch({m}, a = 1L, b = 2L)",
];

/// Typing-mode directives (never strict — see the module doc).
const DIRECTIVES: &[&str] = &["# typing: on", "# typing: off"];

fn iterations() -> usize {
    std::env::var("FUZZ_ITERS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(250)
}

fn generate_program(rng: &mut SplitMix64) -> String {
    let mut lines = Vec::new();
    if rng.chance(1, 6) {
        lines.push(DIRECTIVES[rng.below(DIRECTIVES.len())].to_owned());
        lines.push(String::new());
    }
    let item_count = 1 + rng.below(7);
    for _ in 0..item_count {
        let template = TEMPLATES[rng.below(TEMPLATES.len())];
        let defined = NAMES[rng.below(NAMES.len())];
        let mentioned = NAMES[rng.below(NAMES.len())];
        lines.push(template.replace("{n}", defined).replace("{m}", mentioned));
    }
    lines.join("\n")
}

fn run_arm(kind: DocumentKind, document_path: &str, budget: usize, seed: u64) {
    let mut rng = SplitMix64(seed);
    let mut deficit_rollup: BTreeMap<String, usize> = BTreeMap::new();
    let mut compared = 0usize;
    let mut matching = 0usize;
    let mut oracle_panics = 0usize;
    let mut divergences = 0usize;
    let mut failures = String::new();

    for _ in 0..budget {
        let source = generate_program(&mut rng);
        let legacy = std::panic::catch_unwind(|| legacy_findings(&source, document_path, false));
        let legacy = match legacy {
            Ok((findings, syntax_errors)) => {
                assert!(
                    !syntax_errors,
                    "the generator emitted a program the oracle cannot parse:\n{source}"
                );
                findings
            }
            Err(_) => {
                oracle_panics += 1;
                continue;
            }
        };
        // A panic in the new stack is a bug this arm just found — attach the
        // input (a bare checker panic names no source) and fail loudly.
        let new = std::panic::catch_unwind(|| new_findings(&source, kind, false));
        let new = match new {
            Ok(findings) => findings,
            Err(payload) => {
                eprintln!("---- new stack panicked ({kind:?}) ----\n{source}\n----");
                std::panic::resume_unwind(payload);
            }
        };
        compared += 1;
        let facts = new_stack_facts(&source, kind);
        let new = filter_legacy_slot_tolerance(new, &legacy, &facts, &mut deficit_rollup);
        let new = filter_unknown_type_additions(new, &mut deficit_rollup);
        let (legacy, accepted) =
            filter_oracle_deficits(legacy, &new, Some(&facts), &mut deficit_rollup);
        let (legacy, new) =
            filter_model_divergences(legacy, new, &facts, &accepted, &mut deficit_rollup);
        let mut wording = Vec::new();
        if findings_match(&legacy, &new, &mut wording) {
            matching += 1;
            continue;
        }
        divergences += 1;
        if divergences <= 10 {
            let _ = writeln!(failures, "==== program ====\n{source}");
            for finding in &legacy {
                if !new.contains(finding) {
                    let _ = writeln!(
                        failures,
                        "  legacy only: {}..{} [{}] {}",
                        finding.1, finding.2, finding.0, finding.3
                    );
                }
            }
            for finding in &new {
                if !legacy.contains(finding) {
                    let _ = writeln!(
                        failures,
                        "  new only:    {}..{} [{}] {}",
                        finding.1, finding.2, finding.0, finding.3
                    );
                }
            }
        }
    }

    let accepted: usize = deficit_rollup.values().sum();
    println!(
        "fuzz differential ({kind:?}): {matching}/{compared} programs match, {accepted} oracle-deficit findings accepted, {oracle_panics} oracle panics skipped"
    );
    assert!(
        divergences == 0,
        "{divergences} diverging program(s) (first 10 shown):\n{failures}"
    );
}

#[test]
fn fuzz_differential_package() {
    run_arm(
        DocumentKind::Package,
        "/pkg/R/case.R",
        iterations(),
        0xD1FF_0001,
    );
}

#[test]
fn fuzz_differential_script() {
    run_arm(
        DocumentKind::Script,
        "/pkg/scripts/case.R",
        iterations(),
        0xD1FF_0002,
    );
}

#[test]
#[ignore = "long fuzz run for nightly/manual use"]
fn fuzz_differential_deep() {
    run_arm(
        DocumentKind::Package,
        "/pkg/R/case.R",
        iterations() * 20,
        0xD1FF_0003,
    );
    run_arm(
        DocumentKind::Script,
        "/pkg/scripts/case.R",
        iterations() * 20,
        0xD1FF_0004,
    );
}
