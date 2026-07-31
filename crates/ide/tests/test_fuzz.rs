//! IDE fuzz harness, per the pipeline-wide doctrine. On every input each
//! feature runs at a sample of byte offsets — every token boundary plus a
//! coarse stride, so off-boundary and mid-character positions are still
//! covered — and at each one must not panic and must keep every range it
//! produces inside the text that range indexes. Hover is additionally checked
//! for determinism within one database.
//!
//! None of these features is memoized, so a second call for its own sake costs
//! as much as the first; the budget goes on offsets and inputs instead, and the
//! range checks are free once a result is in hand.
//!
//! `FUZZ_ITERS` scales the mutation budget (default 300 — every iteration
//! sweeps a whole input, so inputs stay small).

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

/// Runs every feature at a sample of positions in `source`, checking that every
/// range it hands the editor lies inside the text that range indexes. Panics
/// propagate.
fn sweep(source: &str) {
    let db = RootDatabase::default();
    semantics::stubs::install_shipped_stubs(&db);
    let file = SourceFile::new(&db, source.to_owned(), DocumentKind::Package);
    let files = ProjectFiles::new(&db, vec![file]);
    for offset in sample_offsets(source) {
        let offset = TextSize::from(offset as u32);
        let at = |what: &str| format!("{what} in {source:?} at {offset:?}");

        let hover = ide::hover(&db, files, file, offset);
        assert_eq!(
            hover,
            ide::hover(&db, files, file, offset),
            "{}",
            at("non-deterministic hover")
        );
        if let Some(hover) = &hover {
            check_range(&db, file, hover.range, &at("hover"));
        }

        let _ = ide::hover_debug(&db, file, offset);

        let definition = ide::definition(&db, files, file, offset);
        check_target(&db, definition.as_ref(), &at("definition"));

        let references = ide::references(&db, files, file, offset, true);
        for occurrence in &references {
            check_range(&db, occurrence.file, occurrence.range, &at("reference"));
        }

        let rename = ide::rename(&db, files, file, offset);
        for occurrence in rename.iter().flatten() {
            check_range(&db, occurrence.file, occurrence.range, &at("rename edit"));
        }

        // A signature's parameter spans index its own rendered label, not the
        // file, and the editor slices the label with them.
        for signature in ide::signature_help(&db, file, offset)
            .iter()
            .flat_map(|help| &help.signatures)
        {
            for parameter in &signature.parameters {
                assert!(
                    usize::from(parameter.end()) <= signature.label.len(),
                    "{}: parameter span {parameter:?} escapes label {:?}",
                    at("signature help"),
                    signature.label
                );
            }
        }

        for item in ide::completion(&db, files, file, offset)
            .iter()
            .flat_map(|result| &result.items)
        {
            assert!(
                !item.label.is_empty(),
                "{}: empty completion label",
                at("completion")
            );
        }

        let type_definition = ide::type_definition(&db, files, file, offset);
        check_target(&db, type_definition.as_ref(), &at("type definition"));

        let selection = syntax::TextRange::new(offset, offset);
        let actions = ide::code_actions(&db, file, selection);
        for action in &actions {
            for edit in &action.edits {
                check_range(&db, file, edit.range, &at("code action edit"));
            }
        }
    }

    let hints = ide::inlay_hints(&db, file, None);
    for hint in &hints {
        assert!(
            usize::from(hint.offset) <= source.len(),
            "inlay hint offset {:?} escapes {source:?}",
            hint.offset
        );
    }
    for symbol in ide::document_symbols(&db, file) {
        check_symbol(&db, file, &symbol);
    }
    let _ = ide::workspace_symbols(&db, files, "a");
}

/// Token boundaries plus a coarse stride over the rest. The boundaries are where
/// the interesting position decisions happen (end-inclusive containment, the
/// item-end touch) and the stride keeps the off-boundary and mid-character cases
/// that have found real panics — while not paying for every byte, which is what
/// made this the most expensive test in the repository: the per-call work is over
/// the shipped stub corpus, so it barely varies with file size.
fn sample_offsets(source: &str) -> Vec<usize> {
    const STRIDE: usize = 7;
    let mut offsets = vec![0usize];
    let mut at = 0usize;
    for token in syntax::lex(source).0 {
        at += usize::from(token.len);
        offsets.push(at);
    }
    offsets.extend((0..=source.len()).step_by(STRIDE));
    offsets.push(source.len());
    offsets.sort_unstable();
    offsets.dedup();
    offsets.retain(|&offset| offset <= source.len());
    offsets
}

/// Every range a feature returns goes straight to the editor, so it must lie
/// inside the file it names — a stale or out-of-bounds range is a client-side
/// error at best and a lost edit at worst.
fn check_range(db: &RootDatabase, file: SourceFile, range: syntax::TextRange, what: &str) {
    let length = file.text(db).len();
    assert!(
        usize::from(range.end()) <= length,
        "{what}: range {range:?} escapes its file (length {length})"
    );
}

fn check_target(db: &RootDatabase, target: Option<&ide::DefinitionTarget>, what: &str) {
    // A stub target's range indexes a stub source rather than a project file,
    // and the harness installs no stub texts to measure it against.
    if let Some(ide::DefinitionTarget::Project(navigation)) = target {
        check_range(db, navigation.file, navigation.range, what);
    }
}

fn check_symbol(db: &RootDatabase, file: SourceFile, symbol: &ide::DocumentSymbol) {
    check_range(db, file, symbol.range, "document symbol");
    check_range(db, file, symbol.selection, "document symbol selection");
    for child in &symbol.children {
        check_symbol(db, file, child);
    }
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
