fn main() {
    let mut parser = typing::new_parser();

    let exprs = vec![r#"4 + 4"#, r#""foo" + 4"#, r#"fn(foo)"#];
    for expr in exprs {
        let tree = typing::parse(&mut parser, expr, None);
        let result = typing::check(tree.root_node());
        match result {
            Ok(typ) => eprintln!("expr: {expr}\ntype: {typ:?}\n"),
            Err(err) => eprintln!(
                "expr: {expr}\nrange: {:?}\nerror: {}\n",
                err.range, err.message
            ),
        }
    }

    // TODO: use miette
}
