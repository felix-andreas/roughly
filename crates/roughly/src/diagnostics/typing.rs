use {crate::lsp_types::Diagnostic, ropey::Rope, tree_sitter::Node};

pub fn analyze(_node: Node, _rope: &Rope) -> Vec<Diagnostic> {
    vec![]
}
