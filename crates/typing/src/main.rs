fn main() {
    let mut parser = typing::new_parser();

    let exprs = vec![r#"4 + 4"#, r#""foo" + 4"#, r#"fn(foo)"#];
    for expr in exprs {
        let tree = typing::parse(&mut parser, expr, None);
        let result = typing::check(tree.root_node());
        match result {
            Ok(typ) => eprintln!("expr: {expr}\ntype: {typ:?}\n"),
            Err((node, err)) => eprintln!("expr: {expr}\nnode: {node:?}\nerror: {}\n", err.0),
        }
    }

    // TODO: use miette
}
