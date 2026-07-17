//! Fuzz + property harness, run against every parser increment from its first
//! commit.
//!
//! Invariants, checked on every input:
//!   1. never panic (any panic fails the test);
//!   2. lossless — the tree reprints byte-for-byte to the input;
//!   3. the lexed token lengths cover the input exactly.
//!
//! Inputs come from three generators: random bytes, token soup from an R-shaped
//! alphabet, and mutations of seed snippets. `FUZZ_ITERS` scales the run
//! (default 1500 per generator); runs are deterministic per seed.

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
    "x <- 1L",
    "f <- function(x, y = 2) x + y",
    "if (a > 1) b else c",
    "for (i in 1:10) { total <- total + i }",
    "while (TRUE) break",
    "repeat next",
    "lst$field[[2]]$name",
    "pkg::fn(a, b = 2, ...)",
    "m[, 1]",
    "x |> f() |> g(1)",
    "y ~ a + b",
    "p <- \\(x) x^2",
    "s <- \"a string with \\\"escapes\\\"\"",
    "r <- r\"(raw \\ string)\"",
    "z <- 2i + 0x1FL",
    "`odd name` <- 5",
    "a %in% b %o% c",
    "#: fn(x: integer) -> double\nf <- function(x) x / 2",
    "#: @type Point {list[double]}\n#: @strict\np <- list(1, 2)",
    "df[df$col > 3, c(\"a\", \"b\")]",
    "x = 5; y = 6ate; z",
    "if (a) { b } else { c }",
    "tryCatch(expr, error = function(e) NULL)",
    "-2^2 + (1)",
    "x <<- y ->> z",
    "d <- data$frame@slot::name",
];

const ALPHABET: &[&str] = &[
    "x", "value", "f", "(", ")", "{", "}", "[", "]", "[[", ",", ";", "\n", " ", "<-", "=", "+",
    "-", "*", "/", "^", "if", "else", "for", "while", "function", "\\", "1", "2.5", "1L", "2i",
    "\"s\"", "'c'", "%in%", "%%", "|>", "|", "&&", "~", "?", "$", "@", "::", ":", "`n`", "...",
    "..1", "#:", "# comment", "NULL", "TRUE", "NA", "Inf", "r\"(x)\"", "->", "->>", "<<-", "!",
    "==", "<=", ">", "_", "0x1F", "1e5", ".5", "next", "break", "repeat", "in",
];

fn iterations() -> usize {
    std::env::var("FUZZ_ITERS").ok().and_then(|value| value.parse().ok()).unwrap_or(1500)
}

fn check_invariants(input: &str) {
    let parse = syntax::parse(input);
    let reprinted = parse.text();
    assert_eq!(
        reprinted, input,
        "lossless round-trip violated for input {input:?}"
    );

    let (tokens, _errors) = syntax::lex(input);
    let total: u32 = tokens.iter().map(|token| u32::from(token.len)).sum();
    assert_eq!(total as usize, input.len(), "token cover violated for input {input:?}");
    for token in &tokens {
        assert!(u32::from(token.len) > 0, "zero-length token for input {input:?}");
    }
}

#[test]
fn seeds_hold_invariants() {
    for seed in SEEDS {
        check_invariants(seed);
    }
}

#[test]
fn fuzz_random_bytes() {
    let mut rng = SplitMix64(0xDEC0_DE01);
    for _ in 0..iterations() {
        let len = rng.below(120);
        let bytes: Vec<u8> = (0..len).map(|_| (rng.next() & 0xFF) as u8).collect();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        check_invariants(&text);
    }
}

#[test]
fn fuzz_token_soup() {
    let mut rng = SplitMix64(0x50_D4_11);
    for _ in 0..iterations() {
        let pieces = rng.below(40);
        let mut text = String::new();
        for _ in 0..pieces {
            text.push_str(ALPHABET[rng.below(ALPHABET.len())]);
        }
        check_invariants(&text);
    }
}

#[test]
fn fuzz_seed_mutations() {
    let mut rng = SplitMix64(0x5EED_F00D);
    for _ in 0..iterations() {
        let seed = SEEDS[rng.below(SEEDS.len())];
        let mut text = seed.as_bytes().to_vec();
        for _ in 0..(1 + rng.below(4)) {
            match rng.below(3) {
                0 if !text.is_empty() => {
                    let at = rng.below(text.len());
                    text.remove(at);
                }
                1 => {
                    let at = rng.below(text.len() + 1);
                    text.insert(at, (rng.next() & 0x7F) as u8);
                }
                _ if !text.is_empty() => {
                    let at = rng.below(text.len());
                    text[at] = (rng.next() & 0x7F) as u8;
                }
                _ => {}
            }
        }
        let text = String::from_utf8_lossy(&text).into_owned();
        check_invariants(&text);
    }
}

/// Deep nesting must be refused gracefully (diagnostic, no stack overflow).
#[test]
fn deep_nesting_is_refused_not_fatal() {
    let deep = "(".repeat(2000) + "1" + &")".repeat(2000);
    let parse = syntax::parse(&deep);
    assert_eq!(parse.text(), deep);
    assert!(parse.errors().iter().any(|error| error.message.contains("too deep")));
}
