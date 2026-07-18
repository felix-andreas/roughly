//! Formatter fuzz + property harness, run against every formatter increment
//! from its first commit.
//!
//! Invariants, checked on every input:
//!   1. never panic — `format` either produces output or refuses with a
//!      structured error;
//!   2. idempotence — when `format` succeeds, formatting the output again
//!      succeeds and reproduces it byte-for-byte;
//!   3. determinism — the same input formats to the same result.
//!
//! Generators mirror the syntax-crate harness: valid-program seeds, byte-level
//! seed mutations, token soup from an R-shaped alphabet, random bytes, and a
//! corpus arm over real R files when the corpus is fetched. Byte noise mostly
//! exercises the refusal path; the seeds and corpus files exercise the
//! formatter body.
//!
//! `FUZZ_ITERS` scales the per-generator budget (default 1500); runs are
//! deterministic per seed. `fuzz_deep` multiplies everything for
//! nightly/manual runs.

use format::{Config, format};
use std::path::PathBuf;

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
    "x <- 1L\n",
    "f <- function(x, y = 2) x + y\n",
    "if (a > 1) b else c\n",
    "for (i in 1:10) {\n  total <- total + i\n}\n",
    "while (TRUE) break\n",
    "lst$field[[2]]$name\n",
    "pkg::fn(a, b = 2, ...)\n",
    "m[, 1]\n",
    "x |> f() |> g(1)\n",
    "p <- \\(x) x^2\n",
    "s <- \"a string with \\\"escapes\\\"\"\n",
    "s2 <- 'single'\n",
    "r <- r\"(raw \\ string)\"\n",
    "`odd name` <- 5\n",
    "a %in% b %o% c\n",
    "#: fn(x: integer) -> double\nf <- function(x) x / 2\n",
    "#: @type Point {list[double]}\n#: @strict\np <- list(1, 2)\n",
    "#: list{\n#:   a: integer\n#: }\nx <- list(a = 1L)\n",
    "x <- 1 #: integer\n",
    "df[df$col > 3, c(\"a\", \"b\")]\n",
    "x = 5; y = 6; z\n",
    "if (a) {\n  b\n} else {\n  c\n}\n",
    "tryCatch(expr, error = function(e) NULL)\n",
    "call(\n  first,\n  second = 2\n)\n",
    "call(first,\n     second)\n",
    "f(x) # trailing comment\n",
    "# a leading comment\nvalue\n",
    "#foo\n#' roxygen\n#!shebang\n#| echo: false\n## banner\n",
    "m <- rbind(c(1,  2),\n           c(30, 4)) # fmt: skip\n",
    "# fmt: off\nweird   <-  1\n# fmt: on\nplain <- 2\n",
    "switch(t, NULL = , chr = 1)\n",
    "names(x) <- c(\"a\")\n",
    "x[[y[1]]]\n",
    "repeat {\n  next\n}\n",
    "{\n  1; 2\n  3\n}\n",
];

const ALPHABET: &[&str] = &[
    "x",
    "value",
    "f",
    "(",
    ")",
    "{",
    "}",
    "[",
    "]",
    "[[",
    ",",
    ";",
    "\n",
    " ",
    "<-",
    "=",
    "+",
    "-",
    "*",
    "/",
    "^",
    "if",
    "else",
    "for",
    "while",
    "function",
    "\\",
    "1",
    "2.5",
    "1L",
    "\"s\"",
    "'c'",
    "%in%",
    "|>",
    "&&",
    "~",
    "$",
    "@",
    "::",
    ":",
    "`n`",
    "...",
    "#:",
    "# comment",
    "# fmt: skip",
    "# fmt: off",
    "# fmt: on",
    "NULL",
    "TRUE",
    "r\"(x)\"",
    "->",
    "<<-",
    "!",
    "==",
    "next",
    "break",
    "repeat",
    "in",
    "@type",
    "fn",
    "list[",
    "list{",
    "\r\n",
];

fn iterations() -> usize {
    std::env::var("FUZZ_ITERS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1500)
}

/// The invariant battery for one input: no panic, deterministic, and
/// idempotent whenever formatting succeeds.
fn check_invariants(input: &str) {
    let first = format(input, Config::default());
    let again = format(input, Config::default());
    assert_eq!(first, again, "non-deterministic format for input {input:?}");

    if let Ok(output) = first {
        match format(&output, Config::default()) {
            Ok(second) => assert_eq!(second, output, "format not idempotent for input {input:?}"),
            Err(error) => panic!(
                "formatted output no longer formats for input {input:?}: {error} (output {output:?})"
            ),
        }
    }
}

#[test]
fn seeds_hold_invariants() {
    for seed in SEEDS {
        assert!(
            format(seed, Config::default()).is_ok(),
            "seed unexpectedly refused: {seed:?}"
        );
        check_invariants(seed);
    }
}

#[test]
fn fuzz_random_bytes() {
    run_random_bytes(iterations());
}

#[test]
fn fuzz_token_soup() {
    run_token_soup(iterations());
}

#[test]
fn fuzz_seed_mutations() {
    run_seed_mutations(iterations());
}

/// Real corpus files (when fetched): every file either refuses with a
/// structured error or formats idempotently.
#[test]
fn fuzz_corpus_seeded() {
    let Some(files) = corpus_sample(48) else {
        eprintln!("fuzz_corpus_seeded: corpus not fetched; skipping");
        return;
    };
    for source in &files {
        check_invariants(source);
    }
    // Byte mutations over real files reach refusal/verbatim edges that clean
    // sources never hit.
    let mut rng = SplitMix64(0xF0_44_A7);
    let budget = iterations() / 3;
    for _ in 0..budget {
        let source = &files[rng.below(files.len())];
        let mut bytes = source.as_bytes().to_vec();
        for _ in 0..(1 + rng.below(6)) {
            if bytes.is_empty() {
                break;
            }
            match rng.below(3) {
                0 => {
                    let at = rng.below(bytes.len());
                    bytes.remove(at);
                }
                1 => {
                    let at = rng.below(bytes.len() + 1);
                    bytes.insert(at, (rng.next() & 0x7F) as u8);
                }
                _ => {
                    let at = rng.below(bytes.len());
                    bytes[at] = (rng.next() & 0x7F) as u8;
                }
            }
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        check_invariants(&text);
    }
}

/// The everything-at-once deep run for nightly/manual use:
/// `FUZZ_ITERS=50000 cargo test -p format --release --test test_fuzz -- --ignored fuzz_deep`
#[test]
#[ignore = "deep fuzz; run explicitly with a large FUZZ_ITERS"]
fn fuzz_deep() {
    let budget = iterations().max(20_000);
    run_random_bytes(budget);
    run_token_soup(budget);
    run_seed_mutations(budget);
}

fn run_random_bytes(budget: usize) {
    let mut rng = SplitMix64(0xF0_12_34);
    for _ in 0..budget {
        let len = rng.below(160);
        let bytes: Vec<u8> = (0..len).map(|_| (rng.next() & 0xFF) as u8).collect();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        check_invariants(&text);
    }
}

fn run_token_soup(budget: usize) {
    let mut rng = SplitMix64(0xF0_50_42);
    for _ in 0..budget {
        let pieces = rng.below(48);
        let mut text = String::new();
        for _ in 0..pieces {
            text.push_str(ALPHABET[rng.below(ALPHABET.len())]);
            if rng.below(3) == 0 {
                text.push(' ');
            }
        }
        check_invariants(&text);
    }
}

fn run_seed_mutations(budget: usize) {
    let mut rng = SplitMix64(0xF0_5E_ED);
    for _ in 0..budget {
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
                    let to = (from + rng.below(16)).min(donor.len());
                    let at = rng.below(text.len() + 1);
                    for (offset, &byte) in donor[from..to].iter().enumerate() {
                        text.insert(at + offset, byte);
                    }
                }
            }
        }
        let text = String::from_utf8_lossy(&text).into_owned();
        check_invariants(&text);
    }
}

/// A bounded sample of real corpus files, spread deterministically.
fn corpus_sample(count: usize) -> Option<Vec<String>> {
    let root = {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = std::env::var("ROUGHLY_CORPUS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| manifest.join("../../corpus"));
        candidate.is_dir().then_some(candidate)?
    };
    let mut files = Vec::new();
    let mut stack = vec![root.join("r-base"), root.join("cran")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "R") {
                files.push(path);
            }
        }
    }
    files.sort();
    let step = (files.len() / count.max(1)).max(1);
    let sample: Vec<String> = files
        .iter()
        .step_by(step)
        .take(count)
        .filter_map(|path| std::fs::read(path).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .collect();
    (!sample.is_empty()).then_some(sample)
}
