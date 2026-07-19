//! Coverage-guided fuzzing of `syntax::parse`: every UTF-8 input must hold
//! the full parse invariant battery (lossless round-trip, token cover, tree
//! geometry, bounded well-formed errors, determinism).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        syntax::testing::check_parse_invariants(input);
    }
});
