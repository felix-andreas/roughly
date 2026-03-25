use {
    ropey::Rope,
    tree_sitter::{Parser, Tree},
    typing::{AnalysisState, CheckResult},
};

pub fn new_parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_r::LANGUAGE.into())
        .expect("Error loading R parser");
    parser
}

pub fn parse_source(parser: &mut Parser, source: &str) -> Tree {
    parser
        .parse(source, None)
        .expect("tree-sitter failed to produce a syntax tree")
}

pub fn check_source(
    source: &str,
    parser: &mut Parser,
    analysis_state: &mut AnalysisState,
) -> CheckResult {
    let tree = parse_source(parser, source);
    let rope = Rope::from_str(source);
    typing::check(tree.root_node(), &rope, analysis_state)
}
