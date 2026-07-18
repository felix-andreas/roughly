//! IDE fuzz harness, per the pipeline-wide doctrine: every feature runs at
//! EVERY byte offset of each input without panicking, and twice at the same
//! offset with identical results (determinism within one database).
//!
//! `FUZZ_ITERS` scales the mutation budget (default 300 — every iteration
//! sweeps all positions, so inputs stay small).

use semantics::{DocumentKind, ProjectFiles, RootDatabase, SourceFile};
use syntax::TextSize;

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
}

const SEEDS: &[&str] = &[
    "x <- 1L\ny <- x + 1\n",
    "f <- function(a, b = 2) a + b\ng <- function() f(1)\n",
    "#: fn(x: integer) -> double\nhalf <- function(x) x / 2\n",
    "make <- function() {\n  total <- 0\n  add <- function(n) total <<- total + n\n  add\n}\n",
    "lst <- list(a = 1, b = \"s\")\nvalue <- lst$a\n",
    "for (i in 1:10) print(i)\n",
    "if (x > 1) y <- 2 else y <- 3\n",
    "`odd name` <- 5\nuse <- `odd name`\n",
    "broken <- function( {\n",
    "s <- \"unterminated\n",
];

fn iterations() -> usize {
    std::env::var("FUZZ_ITERS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(300)
}

/// Runs every feature at every position of `source`; panics propagate.
fn sweep(source: &str) {
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let file = SourceFile::new(&db, source.to_owned(), DocumentKind::Package);
    let files = ProjectFiles::new(&db, vec![file]);
    for offset in 0..=source.len() {
        let offset = TextSize::from(offset as u32);
        let hover = ide::hover(&db, file, offset);
        assert_eq!(
            hover,
            ide::hover(&db, file, offset),
            "non-deterministic hover in {source:?} at {offset:?}"
        );
        let _ = ide::definition(&db, files, file, offset);
        let _ = ide::references(&db, file, offset);
    }
    let _ = ide::document_symbols(&db, file);
}

#[test]
fn seeds_never_panic() {
    for seed in SEEDS {
        sweep(seed);
    }
}

#[test]
fn fuzz_seed_mutations() {
    let mut rng = SplitMix64(0x01DE_F022);
    for _ in 0..iterations() {
        let seed = SEEDS[rng.below(SEEDS.len())];
        let mut text = seed.as_bytes().to_vec();
        for _ in 0..(1 + rng.below(4)) {
            match rng.below(4) {
                0 if !text.is_empty() => {
                    let at = rng.below(text.len());
                    text.remove(at);
                }
                1 => {
                    let at = rng.below(text.len() + 1);
                    text.insert(at, (rng.next() & 0x7F) as u8);
                }
                2 if !text.is_empty() => {
                    let at = rng.below(text.len());
                    text[at] = (rng.next() & 0x7F) as u8;
                }
                _ => {
                    let donor = SEEDS[rng.below(SEEDS.len())].as_bytes();
                    let from = rng.below(donor.len());
                    let to = (from + rng.below(24)).min(donor.len());
                    let at = rng.below(text.len() + 1);
                    for (index, &byte) in donor[from..to].iter().enumerate() {
                        text.insert(at + index, byte);
                    }
                }
            }
        }
        let text = String::from_utf8_lossy(&text).into_owned();
        sweep(&text);
    }
}

/// The everything-at-once deep run for nightly/manual use:
/// `FUZZ_ITERS=5000 cargo test -p ide --release --test test_fuzz -- --ignored fuzz_deep`
#[test]
#[ignore = "deep fuzz; run explicitly with a large FUZZ_ITERS"]
fn fuzz_deep() {
    let mut rng = SplitMix64(0x01DE_F023);
    for _ in 0..iterations().max(5000) {
        let pieces = 1 + rng.below(12);
        let mut text = String::new();
        for _ in 0..pieces {
            text.push_str(SEEDS[rng.below(SEEDS.len())]);
        }
        sweep(&text);
    }
}
