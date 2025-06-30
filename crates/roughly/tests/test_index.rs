use {
    async_lsp::lsp_types::DocumentSymbol,
    indoc::indoc,
    ropey::Rope,
    roughly::{index, lsp_types::SymbolKind, tree},
    tree_sitter::Node,
};

fn setup(text: &str, nested: bool) -> Vec<DocumentSymbol> {
    let rope = Rope::from_str(text);
    let tree = tree::parse_rope(&mut tree::new_parser(), &rope, None);
    index::index(tree.root_node(), &rope, nested, false)
}

fn setup_nested(text: &str) -> Vec<DocumentSymbol> {
    setup(text, true)
}

fn setup_flat(text: &str) -> Vec<DocumentSymbol> {
    setup(text, false)
}

#[test]
fn assignments() {
    let text = indoc! {r#"
		foo <- function(a, b = True) {
			a <- TRUE
			b <- FALSE
		}
		bar <- \(x, y, z) {
			a <- 1
			b <- "foo"
		}
		baz <- { "foo"; 3.14 }
	"#};

    {
        let symbols = setup_nested(text);
        assert_eq!(symbols.len(), 3);

        assert_eq!(symbols[0].name, "foo");
        assert_eq!(symbols[0].kind, SymbolKind::FUNCTION);
        {
            let children = symbols[0].children.as_ref().unwrap();
            assert_eq!(children[0].name, "a");
            assert_eq!(children[0].kind, SymbolKind::BOOLEAN);
            assert_eq!(children[1].name, "b");
            assert_eq!(children[1].kind, SymbolKind::BOOLEAN);
        }

        assert_eq!(symbols[1].name, "bar");
        assert_eq!(symbols[1].kind, SymbolKind::FUNCTION);
        {
            let children = symbols[1].children.as_ref().unwrap();
            assert_eq!(children[0].name, "a");
            assert_eq!(children[0].kind, SymbolKind::NUMBER);
            assert_eq!(children[1].name, "b");
            assert_eq!(children[1].kind, SymbolKind::STRING);
        }

        assert_eq!(symbols[2].name, "baz");
        assert_eq!(symbols[2].kind, SymbolKind::VARIABLE);
    }

    {
        let symbols = setup_flat(text);
        assert_eq!(symbols.len(), 3);

        assert_eq!(symbols[0].name, "foo");
        assert_eq!(symbols[0].kind, SymbolKind::FUNCTION);
        assert_eq!(symbols[0].children, None);

        assert_eq!(symbols[1].name, "bar");
        assert_eq!(symbols[1].kind, SymbolKind::FUNCTION);
        assert_eq!(symbols[1].children, None);

        assert_eq!(symbols[2].name, "baz");
        assert_eq!(symbols[2].kind, SymbolKind::VARIABLE);
        assert_eq!(symbols[2].children, None);
    }
}

#[test]
fn s4_set_class() {
    let symbols = setup_nested(indoc! {r#"
		setClass(
			"Person",
			slots = c(
				name = "character",
				age = "numeric"
			)
		)
		methods::setClass(
			Class = "Car",
			slots = c(
				name = "character"
			)
		)
	"#});

    assert_eq!(symbols.len(), 2);

    assert_eq!(symbols[0].name, "Person");
    assert_eq!(symbols[0].kind, SymbolKind::CLASS);
    // {
    //     let children = symbols[0].children.as_ref().unwrap();
    //     assert_eq!(children[0].name, "name");
    //     assert_eq!(children[0].kind, SymbolKind::PROPERTY);
    //     assert_eq!(children[1].name, "age");
    //     assert_eq!(children[1].kind, SymbolKind::PROPERTY);
    // }

    assert_eq!(symbols[1].name, "Car");
    assert_eq!(symbols[1].kind, SymbolKind::CLASS);
    // {
    //     let children = symbols[1].children.as_ref().unwrap();
    //     assert_eq!(children[0].name, "name");
    //     assert_eq!(children[0].kind, SymbolKind::PROPERTY);
    // }
}

#[test]
fn s4_set_generic() {
    let symbols = setup_nested(indoc! {r#"
		setGeneric("foo", function(x) standardGeneric("foo"))
		methods::setGeneric(name = "bar<-", def = function(x, value) standardGeneric("bar<-"))
	"#});

    assert_eq!(symbols.len(), 2);

    assert_eq!(symbols[0].name, "foo");
    assert_eq!(symbols[0].kind, SymbolKind::INTERFACE);

    assert_eq!(symbols[1].name, "bar<-");
    assert_eq!(symbols[1].kind, SymbolKind::INTERFACE);

    assert_eq!(symbols.len(), 2);
}

#[test]
fn s4_set_method() {
    let symbols = setup_flat(indoc! {r#"
		setMethod("foo", "Person", function(x) x@foo)
		setMethod(
			"bar<-",
			"Person",
			function(x, value) {
				x@bar <- value
				x
			}
		)
	"#});

    assert_eq!(symbols.len(), 2);

    assert_eq!(symbols[0].name, "foo (Person)");
    assert_eq!(symbols[0].kind, SymbolKind::METHOD);

    assert_eq!(symbols[1].name, "bar<- (Person)");
    assert_eq!(symbols[1].kind, SymbolKind::METHOD);

    assert_eq!(symbols.len(), 2);
}

#[test]
fn s4_set_method_with_signature_arg() {
    let symbols = setup_flat(indoc! {r#"
		setMethod(
			f = "baz",
			signature = "Person",
			definition = function(x) x@baz
		)
	"#});

    assert_eq!(symbols.len(), 1);

    assert_eq!(symbols[0].name, "baz (Person)");
    assert_eq!(symbols[0].kind, SymbolKind::METHOD);
    assert_eq!(symbols.len(), 1);
}

#[test]
fn s4_set_method_with_vector_signature() {
    let symbols = setup_flat(indoc! {r#"
		setMethod(
			"qux",
			c("Person", "Other"),
			function(x, y) x@qux + y@qux
		)
	"#});

    assert_eq!(symbols.len(), 1);

    assert_eq!(symbols[0].name, "qux (Person, Other)");
    assert_eq!(symbols[0].kind, SymbolKind::METHOD);
    assert_eq!(symbols.len(), 1);
}

#[test]
fn s4_set_method_with_named_signature() {
    let symbols = setup_flat(indoc! {r#"
		setMethod(
			f = "foo",
			signature = list(x = "Person", y = "Other"),
			definition = function(x, y) x@foo + y@foo
		)
	"#});

    assert_eq!(symbols.len(), 1);

    assert_eq!(symbols[0].name, "foo (Person, Other)");
    assert_eq!(symbols[0].kind, SymbolKind::METHOD);
    assert_eq!(symbols.len(), 1);
}

#[test]
fn test_r6_class() {
    let symbols = setup_flat(indoc! {r#"
        Person <- R6Class("Person",
            public = list(
                name = NULL,
                age = NULL,
                initialize = function(name, age) {
                    self$name <- name
                    self$age <- age
                },
                greet = function() {
                    cat(paste("Hello, my name is", self$name))
                },
                say_age = function() {
                    cat(paste("I am", self$age, "years old"))
                },
                .hidden = NULL
            ),
            private = list(
                secret = NULL,
                password = NULL,
                reveal_secret = function() {
                    cat(self$secret)
                }
            ),
            active = list(
                full_name = function(value) {
                    if (missing(value)) paste(self$name, "Smith") else self$name <- value
                }
            ),
            inherit = AnotherClass,
            portable = TRUE,
            cloneable = FALSE,
            lock_class = TRUE,
            lock_objects = FALSE
        )

        Car <- R6::R6Class(
            "Person",
            list(
                length = NULL,
                drive = function() {}
            )
        )
    "#});

    let assert = |members: &[DocumentSymbol], name: &str, kind: SymbolKind| {
        assert!(
            members
                .iter()
                .any(|member| member.name == name && member.kind == kind),
        );
    };

    assert_eq!(symbols.len(), 2);

    // Person
    assert_eq!(symbols[0].name, "Person");
    assert_eq!(symbols[0].kind, SymbolKind::CLASS);

    let members = symbols[0].children.as_ref().unwrap();

    // Public properties and methods
    assert(members, "name", SymbolKind::FIELD);
    assert(members, "age", SymbolKind::FIELD);
    assert(members, "initialize", SymbolKind::METHOD);
    assert(members, "greet", SymbolKind::METHOD);
    assert(members, "say_age", SymbolKind::METHOD);
    assert(members, ".hidden", SymbolKind::FIELD);

    // Private properties and methods
    assert(members, "secret", SymbolKind::FIELD);
    assert(members, "password", SymbolKind::FIELD);
    assert(members, "reveal_secret", SymbolKind::METHOD);

    // Active bindings
    assert(members, "full_name", SymbolKind::PROPERTY);

    // Car
    assert_eq!(symbols[1].name, "Car");
    assert_eq!(symbols[1].kind, SymbolKind::CLASS);

    let members = symbols[1].children.as_ref().unwrap();
    assert(members, "length", SymbolKind::FIELD);
    assert(members, "drive", SymbolKind::METHOD);
}

#[test]
fn get_argument_named_and_positional() {
    let text = indoc! {r#"
        call(
            "alpha",
            second = "beta",
            third = "gamma"
        )
    "#};
    let rope = Rope::from_str(text);
    let tree = tree::parse_rope(&mut tree::new_parser(), &rope, None);
    let arguments = tree
        .root_node()
        .child(0)
        .unwrap()
        .child_by_field_name("arguments")
        .unwrap();

    let extract_content = |node: Node| {
        rope.byte_slice(node.child_by_field_name("content").unwrap().byte_range())
            .to_string()
    };

    // Positional argument
    let argument = index::get_argument(arguments, &rope, "not_found", 0).unwrap();
    assert_eq!(argument.kind(), "string");
    assert_eq!(extract_content(argument), "alpha");

    // Named argument
    let argument = index::get_argument(arguments, &rope, "second", 1).unwrap();
    assert_eq!(argument.kind(), "string");
    assert_eq!(extract_content(argument), "beta");

    // Named argument for gamma
    let argument = index::get_argument(arguments, &rope, "third", 2).unwrap();
    assert_eq!(argument.kind(), "string");
    assert_eq!(extract_content(argument), "gamma");

    // Non-existent named argument
    let arg = index::get_argument(arguments, &rope, "does_not_exist", 10);
    assert!(arg.is_none());
}
