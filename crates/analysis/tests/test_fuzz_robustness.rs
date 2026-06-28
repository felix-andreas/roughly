//! Backlog F2 — seeded, deterministic robustness fuzzer for the parse -> lower -> type-syntax
//! pipeline plus the incremental-edit (stale-range) path.
//!
//! `cargo-fuzz`/libFuzzer is not available in this environment, so this is a self-contained,
//! seeded PRNG harness committed as an ordinary `cargo test`. It runs every CI cycle in a fast,
//! bounded default mode and is meant to catch robustness regressions, not to replace coverage-guided
//! fuzzing.
//!
//! Promotion to a real `cargo-fuzz` target later is intentionally cheap: the per-input work lives in
//! the standalone `check_input` / `check_type_syntax` functions, so a future `fuzz_target!(|data: &[u8]|
//! { if let Ok(s) = std::str::from_utf8(data) { check_input(s); check_type_syntax(s); } })` can call
//! the exact same bodies.
//!
//! Invariants asserted for every generated input (a violation = test failure):
//!   1. NO PANIC anywhere in parse -> lower -> type-syntax parsing.
//!   2. TERMINATION — pathologically deep type strings hit the type-syntax recursion guard
//!      (`TYPE_SYNTAX_RECURSION_LIMIT`) and return an error instead of overflowing the stack.
//!   3. `Document::parse` always yields a tree (never `None`/panic) for any input.
//!   4. NO OUT-OF-BOUNDS ON STALE RANGES — random, possibly out-of-bounds / mid-codepoint edit
//!      ranges driven through `Analysis::edit_document` never panic or index out of bounds (the edit
//!      path validates and returns `Err(AnalysisError::InvalidEditRange)` for malformed ranges).
//!
//! `lower` is bounded by `LOWER_RECURSION_LIMIT` (its own depth guard, added after this fuzzer first
//! found that unguarded R *source* nesting beyond ~300 — e.g. `{{{...{1}...}}}` — overflowed the
//! stack into an uncatchable abort). The generators now nest *above* that guard so the guard path is
//! actively exercised, and `deeply_nested_braces_do_not_abort` pins the regression deterministically.

use {
    analysis::{
        Analysis, CheckConfig, DocumentChange, LintConfig,
        document::Document,
        interner::Interner,
        lower::{LoweringContext, lower},
        text::{TextPosition, TextRange},
        tree::new_parser,
        type_syntax::{parse_annotation_type, parse_surface_type, parse_type_syntax},
    },
    std::path::PathBuf,
};

// Fixed seed + bounded iteration budget for the default CI run. Tuned to finish in well under a
// couple of seconds while still replaying every generator.
const DEFAULT_SEED: u64 = 0x5DEE_CE66_D1CE_F00D;
const INPUT_ITERS: usize = 3000;
const TYPE_ITERS: usize = 3000;
const EDIT_SEQUENCES: usize = 350;

// Soak (`#[ignore]`) budget for manual/nightly deep fuzzing.
const SOAK_ITERS: usize = 5_000_000;

// R-source bracket nesting now ranges above `lower`'s recursion guard (160) so the guard path is
// exercised; it stays below the ~300 stack-overflow boundary that the guard protects against.
const MAX_SOURCE_NESTING: usize = 240;
const DEEP_TYPE_NESTING: usize = 6000;

// Deterministic regression corpus: replayed verbatim before any random fuzzing so a fixed reproducer
// can never silently regress. Seeded with known-nasty shapes even though no *catchable* crash was
// found in parse/lower/type-syntax at these (bounded) depths.
const SEEDS: &[&str] = &[
    "",
    " ",
    "\0",
    "\n\n\n",
    "#:",
    "#: ",
    "#:|",
    "#: fn(",
    "#: list[",
    "#: integer | | character",
    "#: ->",
    "#: <>",
    "#: @new",
    "#: @type Foo {",
    "x <- function(",
    "f(((((",
    "{{{{",
    "]]]]]",
    "a $ @ :: ::: $",
    "'unterminated",
    "\"unterminated",
    "r\"(unterminated raw",
    "r\"(balanced)\"",
    "1L 1.5 0xFF 1e10 1i 0x",
    "x <<- y -> z ->> w",
    "%in% %% %/% %custom%",
    "for while repeat if else function in next break",
    "résumé <- 'naïve façade'",
    "list[integer] | character | NULL",
    "#: fn(x: integer, y: character) -> logical",
    "x[[1]][[2]]$a@b",
];

// ---------------------------------------------------------------------------------------------------
// Per-input checks (the bodies a future `fuzz_target!` would call).
// ---------------------------------------------------------------------------------------------------

/// Drive one source string through parse -> lower and feed its `#:` annotations + the raw text to the
/// type-syntax parser. Panicking, overflowing, or a `None` parse tree all fail the calling test.
fn check_input(source: &str) {
    let mut parser = new_parser().expect("parser should initialize");

    // Invariant 3: tree-sitter must always produce a tree.
    let document = Document::parse(&mut parser, source)
        .unwrap_or_else(|error| panic!("Document::parse returned no tree for {source:?}: {error:?}"));

    // Invariant 1: lowering must not panic. (Source nesting is bounded by the generators below the
    // unguarded-`lower` overflow boundary — see the module-level KNOWN GAP note.)
    let mut context = LoweringContext::new();
    let _module = lower(&document, &mut context);

    // Feed every `#:` annotation line, and the raw source, to the type-syntax parser.
    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("#:") {
            check_type_syntax(rest);
        }
    }
    check_type_syntax(source);
}

/// Drive one string through every public type-syntax entry point. Each call must terminate and return
/// `Ok`/`Err` without panicking or overflowing (the recursion guard turns deep nesting into an error).
fn check_type_syntax(text: &str) {
    let mut interner = Interner::new();
    let _ = parse_type_syntax(text, &mut interner);
    let _ = parse_surface_type(text, &mut interner);
    let _ = parse_annotation_type(text, &mut interner, false);
    let _ = parse_annotation_type(text, &mut interner, true);
}

// ---------------------------------------------------------------------------------------------------
// Seeded PRNG (xorshift64*).
// ---------------------------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Avoid the all-zero state, which xorshift cannot leave.
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }

    fn pick<T: Copy>(&mut self, options: &[T]) -> T {
        options[self.below(options.len())]
    }
}

// ---------------------------------------------------------------------------------------------------
// Generators.
// ---------------------------------------------------------------------------------------------------

// Weighted token alphabet biased toward R syntax. Brackets appear here too, but the generator tracks
// nesting separately so source depth stays bounded.
const R_TOKENS: &[&str] = &[
    "x", "y", "value", "foo", "f", "df", ".x", "x1", "T", "F",
    "1", "1L", "1.5", "0xFF", "1e10", "1i", ".5", "0x", "100L", "NA",
    "NA_integer_", "NA_real_", "NULL", "TRUE", "FALSE", "Inf", "NaN",
    "<-", "<<-", "->", "->>", "=", "==", "!=", "<=", ">=", "<", ">",
    "+", "-", "*", "/", "^", "%%", "%/%", "%in%", "%>%", "%custom%",
    "::", ":::", "$", "@", ":", "|>", "|", "||", "&", "&&", "!", "~", "?",
    "function", "if", "else", "for", "while", "repeat", "in", "next", "break", "return",
    "'str'", "\"str\"", "'unterminated", "\"unterminated", "r\"(raw)\"", "r\"(unterminated",
    "\\(x)", ";", ",", " ", "\n", "\t", "#comment", "café", "λ", "\u{0}",
];

// Type-syntax tokens for the `#:` / standalone-type path.
const TYPE_TOKENS: &[&str] = &[
    "integer", "double", "character", "logical", "complex", "raw", "Any", "Unknown", "NULL",
    "fn", "->", "|", "[", "]", "{", "}", "(", ")", ",", ":", "<", ">", "@new", "@type", "@alias",
    "@trust", "@if-unknown", "list", "data.frame", "Foo", "T", "x", " ", "\n",
];

const ANNOTATION_PREFIXES: &[&str] = &["#: ", "#:", "# ", "#' ", ""];

fn gen_r_source(rng: &mut Rng) -> String {
    let mut out = String::new();
    let mut open: Vec<&str> = Vec::new();
    let token_count = 1 + rng.below(60);

    for _ in 0..token_count {
        // Occasionally emit a whole annotation line.
        if rng.chance(15) {
            out.push('\n');
            out.push_str(rng.pick(ANNOTATION_PREFIXES));
            out.push_str(&gen_type_string(rng, 8));
            out.push('\n');
            continue;
        }

        // Open a bracket, but only while under the safe nesting cap.
        if open.len() < MAX_SOURCE_NESTING && rng.chance(25) {
            let (opener, closer) = rng.pick(&[("(", ")"), ("[", "]"), ("{", "}"), ("[[", "]]")]);
            out.push_str(opener);
            open.push(closer);
            continue;
        }

        // Close a bracket — but only sometimes, so output is frequently unbalanced.
        if !open.is_empty() && rng.chance(40) {
            out.push_str(open.pop().unwrap_or(")"));
            continue;
        }

        out.push_str(rng.pick(R_TOKENS));
    }

    // Leave the remaining brackets open most of the time (unbalanced is the interesting case).
    if rng.chance(30) {
        while let Some(closer) = open.pop() {
            out.push_str(closer);
        }
    }
    out
}

fn gen_type_string(rng: &mut Rng, max_tokens: usize) -> String {
    let count = 1 + rng.below(max_tokens.max(1));
    let mut out = String::new();
    for _ in 0..count {
        out.push_str(rng.pick(TYPE_TOKENS));
    }
    out
}

// Pathologically deep type strings to actively exercise the recursion guard (invariant 2).
fn gen_deep_type(rng: &mut Rng) -> String {
    let depth = 200 + rng.below(DEEP_TYPE_NESTING);
    let (opener, closer) = rng.pick(&[
        ("list[", "]"),
        ("fn(", ")"),
        ("(", ")"),
        ("{", "}"),
    ]);
    // For `|` there is no closer; build a long alternation chain instead.
    if rng.chance(20) {
        return std::iter::repeat("integer | ").take(depth).collect::<String>() + "integer";
    }
    let mut out = String::with_capacity(depth * opener.len() + 8 + depth);
    for _ in 0..depth {
        out.push_str(opener);
    }
    out.push_str("integer");
    // Half the time leave it unterminated to exercise the error path under deep nesting too.
    if rng.chance(50) {
        for _ in 0..depth {
            out.push_str(closer);
        }
    }
    out
}

// Random byte-ish source: arbitrary chars including NUL, control bytes, and non-ASCII/unicode.
fn gen_random_bytes(rng: &mut Rng) -> String {
    let len = rng.below(120);
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        let choice = rng.below(100);
        let ch = if choice < 60 {
            // Printable ASCII.
            char::from((0x20 + rng.below(0x5F) as u8) as u8)
        } else if choice < 75 {
            '\u{0}'
        } else if choice < 85 {
            // Other control bytes.
            char::from(rng.below(0x20) as u8)
        } else {
            // Non-ASCII / multi-byte codepoints.
            rng.pick(&['é', 'λ', '日', '🦀', 'ß', '∑', '\u{200B}', 'Ω'])
        };
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------------------------------------------
// S6: incremental-edit (stale-range) fuzzing.
// ---------------------------------------------------------------------------------------------------

// Random, deliberately stale/out-of-bounds/mid-codepoint edit positions over a document. The harness
// only requires that `edit_document` never panics; it may legitimately return `Err(InvalidEditRange)`.
fn gen_position(rng: &mut Rng) -> TextPosition {
    TextPosition {
        // Lines and columns deliberately reach past any small document.
        line_index: rng.below(12),
        character_index: rng.below(20),
    }
}

fn check_edit_sequence(rng: &mut Rng) {
    let path = PathBuf::from("/workspace/R/fuzz.R");
    let mut analysis = Analysis::new(
        PathBuf::from("/workspace"),
        LintConfig::default(),
        CheckConfig::default(),
    );

    // Initial source includes unicode so columns can land mid-codepoint.
    let initial = format!("x <- 1L # café\nf <- function(a) a + λ\n{}", gen_type_string(rng, 4));
    analysis
        .add_document_from_source(path.clone(), &initial)
        .expect("initial document should parse");

    let edits = 1 + rng.below(24);
    for _ in 0..edits {
        let (mut start, mut end) = (gen_position(rng), gen_position(rng));
        // Sometimes force start > end to exercise the reversed-range rejection.
        if rng.chance(25) {
            std::mem::swap(&mut start, &mut end);
        }
        // Keep inserted text short and bracket-light so edits cannot accumulate into source nesting
        // deep enough to overflow the unguarded `lower` walk.
        let text = match rng.below(6) {
            0 => String::new(),
            1 => "x".to_owned(),
            2 => "café".to_owned(),
            3 => "\n".to_owned(),
            4 => "#: integer".to_owned(),
            _ => "value <- 1L".to_owned(),
        };
        let change = DocumentChange {
            range: TextRange { start, end },
            text,
        };
        // Must not panic; an out-of-bounds/stale range returns Err and is fine.
        let _ = analysis.edit_document(&path, std::slice::from_ref(&change));
    }

    // The document must remain coherent enough to keep lowering without panicking.
    analysis::lower(&mut analysis);
}

// ---------------------------------------------------------------------------------------------------
// Drivers.
// ---------------------------------------------------------------------------------------------------

fn run_fuzz(seed: u64, input_iters: usize, type_iters: usize, edit_sequences: usize) {
    // Always replay the deterministic corpus first.
    for seed_input in SEEDS {
        check_input(seed_input);
        check_type_syntax(seed_input);
    }

    let mut rng = Rng::new(seed);

    for index in 0..input_iters {
        let source = match index % 4 {
            0 | 1 => gen_r_source(&mut rng),
            2 => gen_random_bytes(&mut rng),
            _ => format!("#: {}\n{}", gen_type_string(&mut rng, 10), gen_r_source(&mut rng)),
        };
        check_input(&source);
    }

    for index in 0..type_iters {
        let text = if index % 5 == 0 {
            gen_deep_type(&mut rng)
        } else {
            gen_type_string(&mut rng, 24)
        };
        check_type_syntax(&text);
    }

    for _ in 0..edit_sequences {
        check_edit_sequence(&mut rng);
    }
}

#[test]
fn fuzz_default() {
    run_fuzz(DEFAULT_SEED, INPUT_ITERS, TYPE_ITERS, EDIT_SEQUENCES);
}

/// Long-running deep-fuzz variant for manual/nightly use:
/// `cargo test -p analysis --test test_fuzz_robustness -- --ignored --nocapture fuzz_soak`
#[test]
#[ignore]
fn fuzz_soak() {
    run_fuzz(DEFAULT_SEED, SOAK_ITERS, SOAK_ITERS, SOAK_ITERS / 1000);
}

// Deterministic regression for the `lower` recursion guard. ~400 balanced `{ }` blocks carry no
// syntax error, so lowering proceeds — and before the guard this overflowed the stack into an
// uncatchable abort. With the guard, lowering completes (this test returning at all proves no abort)
// and emits a "nested too deeply" diagnostic instead of recursing further.
#[test]
fn deeply_nested_braces_do_not_abort() {
    let depth = 400;
    let source = format!("{}1{}", "{".repeat(depth), "}".repeat(depth));

    let mut parser = new_parser().expect("parser");
    let document = Document::parse(&mut parser, &source).expect("tree for balanced braces");

    let mut context = LoweringContext::new();
    let _module = lower(&document, &mut context);
    let diagnostics = context.take_diagnostics();

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("nested too deeply")),
        "deeply nested braces must lower to a 'nested too deeply' diagnostic, got: {:?}",
        diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// Deterministic regression for the S6 stale-range fix: malformed edit ranges must be rejected with an
// error, and a valid range must still apply. Before the fix, an out-of-bounds line panicked deep in
// the rope (`Line index out of bounds`).
#[test]
fn stale_edit_ranges_are_rejected_without_panic() {
    use analysis::AnalysisError;

    let path = PathBuf::from("/workspace/R/main.R");
    let make = || {
        let mut analysis = Analysis::new(
            PathBuf::from("/workspace"),
            LintConfig::default(),
            CheckConfig::default(),
        );
        // Multi-byte content so a mid-codepoint column is reachable.
        analysis
            .add_document_from_source(path.clone(), "x <- 'café'")
            .expect("document should parse");
        analysis
    };
    let position = |line, character| TextPosition {
        line_index: line,
        character_index: character,
    };
    let change = |start, end| DocumentChange {
        range: TextRange { start, end },
        text: "z".to_owned(),
    };

    let invalid_cases = [
        // Line past the document.
        change(position(9, 0), position(9, 1)),
        // Column past the line.
        change(position(0, 999), position(0, 1000)),
        // start > end.
        change(position(0, 5), position(0, 2)),
        // Column landing inside the multi-byte 'é' (it spans bytes 9..=10, so byte 10 is mid-codepoint).
        change(position(0, 10), position(0, 10)),
    ];
    for case in invalid_cases {
        let mut analysis = make();
        let result = analysis.edit_document(&path, std::slice::from_ref(&case));
        assert!(
            matches!(result, Err(AnalysisError::InvalidEditRange(_))),
            "expected InvalidEditRange for {:?}, got {result:?}",
            case.range,
        );
    }

    // A well-formed range still applies.
    let mut analysis = make();
    analysis
        .edit_document(&path, &[change(position(0, 0), position(0, 1))])
        .expect("valid edit should succeed");
}
