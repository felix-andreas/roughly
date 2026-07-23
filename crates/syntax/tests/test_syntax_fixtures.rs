//! Golden-tree fixture suite: each case renders the full lossless tree plus
//! syntax errors. `ROUGHLY_BLESS=1` accepts new output; `FIXTURE_FILTER=id`
//! runs one case.

use std::path::Path;

#[test]
fn syntax_fixtures() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/syntax");
    syntax::testing::run_fixture_suite(&suite, &|source| syntax::parse(source).debug_dump());
}

/// tree-sitter-r's own parser test corpus, converted into the fixture format
/// (scripts/convert-tsr-corpus.rs) with OUR golden trees as expectations; the
/// original sexps are kept beside the fetched corpus for adjudication.
#[test]
fn tree_sitter_r_corpus_fixtures() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/tsr");
    syntax::testing::run_fixture_suite(&suite, &|source| syntax::parse(source).debug_dump());
}
