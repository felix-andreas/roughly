//! Golden-tree fixture suite: each case renders the full lossless tree plus
//! syntax errors. `ROUGHLY_BLESS=1` accepts new output; `FIXTURE_FILTER=id`
//! runs one case.

use std::path::Path;

#[test]
fn syntax_fixtures() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/syntax");
    syntax::testing::run_fixture_suite(&suite, &|source| syntax::parse(source).debug_dump());
}
