use tree_sitter::{Parser, Tree};

pub fn new_parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_r::LANGUAGE.into())
        .expect("Error loading R parser");
    parser
}

pub fn parse(parser: &mut Parser, text: impl AsRef<[u8]>, maybe_tree: Option<&Tree>) -> Tree {
    parser
        .parse(text, maybe_tree)
        .expect("tree-sitter failed to produce a syntax tree")
}
