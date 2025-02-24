use {
    crate::{tree, utils},
    std::path::Path,
};

pub fn sexp(path: &Path) -> Result<(), std::io::Error> {
    let text = std::fs::read_to_string(path).unwrap();
    let tree = tree::parse(&text, None);
    println!("{}", utils::format_node(tree.root_node()));
    Ok(())
}
