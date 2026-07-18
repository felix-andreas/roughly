//! Golden formatter fixtures: each case's source formats to the expected
//! block. `ROUGHLY_BLESS=1` accepts new output; `FIXTURE_FILTER=group__case`
//! runs one case. Every case also checks idempotence: formatting the output
//! again must reproduce it.

use format::{Config, format};
use std::path::Path;

fn render(source: &str) -> String {
    match format(source, Config::default()) {
        Ok(output) => match format(&output, Config::default()) {
            Ok(second) if second != output => {
                format!("NOT IDEMPOTENT\n---- first ----\n{output}\n---- second ----\n{second}")
            }
            Ok(_) => output,
            Err(error) => format!("REFORMAT ERROR: {error}\n---- first ----\n{output}"),
        },
        Err(error) => format!("ERROR: {error}"),
    }
}

#[test]
fn format_fixtures() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/format");
    syntax::testing::run_fixture_suite(&suite, &render);
}
