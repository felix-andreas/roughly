use {
    indoc::indoc,
    ropey::Rope,
    roughly_legacy::{
        index::{self, Item, ItemInfo},
        tree,
    },
    tree_sitter::Node,
};

fn setup(text: &str, nested: bool) -> Vec<Item> {
    let rope = Rope::from_str(text);
    let tree = tree::parse_rope(&mut tree::new_parser(), &rope, None);
    index::index(tree.root_node(), &rope, nested, false)
}

fn setup_nested(text: &str) -> Vec<Item> {
    setup(text, true)
}

fn setup_flat(text: &str) -> Vec<Item> {
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
        assert_eq!(symbols[0].info, ItemInfo::Function);
        {
            let children = symbols[0].children.as_ref().unwrap();
            assert_eq!(children[0].name, "a");
            assert_eq!(children[0].info, ItemInfo::Bool);
            assert_eq!(children[1].name, "b");
            assert_eq!(children[1].info, ItemInfo::Bool);
        }

        assert_eq!(symbols[1].name, "bar");
        assert_eq!(symbols[1].info, ItemInfo::Function);
        {
            let children = symbols[1].children.as_ref().unwrap();
            assert_eq!(children[0].name, "a");
            assert_eq!(children[0].info, ItemInfo::Float);
            assert_eq!(children[1].name, "b");
            assert_eq!(children[1].info, ItemInfo::String);
        }

        assert_eq!(symbols[2].name, "baz");
        assert_eq!(symbols[2].info, ItemInfo::Unknown);
    }

    {
        let symbols = setup_flat(text);
        assert_eq!(symbols.len(), 3);

        assert_eq!(symbols[0].name, "foo");
        assert_eq!(symbols[0].info, ItemInfo::Function);
        assert!(symbols[0].children.is_none());

        assert_eq!(symbols[1].name, "bar");
        assert_eq!(symbols[1].info, ItemInfo::Function);
        assert!(symbols[1].children.is_none());

        assert_eq!(symbols[2].name, "baz");
        assert_eq!(symbols[2].info, ItemInfo::Unknown);
        assert!(symbols[2].children.is_none());
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
    assert_eq!(symbols[0].info, ItemInfo::S4Class);
    // {
    //     let children = symbols[0].children.as_ref().unwrap();
    //     assert_eq!(children[0].name, "name");
    //     assert_eq!(children[0].kind, SymbolKind::PROPERTY);
    //     assert_eq!(children[1].name, "age");
    //     assert_eq!(children[1].kind, SymbolKind::PROPERTY);
    // }

    assert_eq!(symbols[1].name, "Car");
    assert_eq!(symbols[1].info, ItemInfo::S4Class);
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
    assert_eq!(symbols[0].info, ItemInfo::S4Generic);

    assert_eq!(symbols[1].name, "bar<-");
    assert_eq!(symbols[1].info, ItemInfo::S4Generic);
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

    assert_eq!(symbols[0].name, "foo");
    assert_eq!(
        symbols[0].info,
        ItemInfo::S4Method {
            signature: "Person".into()
        }
    );

    assert_eq!(symbols[1].name, "bar<-");
    assert_eq!(
        symbols[1].info,
        ItemInfo::S4Method {
            signature: "Person".into()
        }
    );
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

    assert_eq!(symbols[0].name, "baz");
    assert_eq!(
        symbols[0].info,
        ItemInfo::S4Method {
            signature: "Person".into()
        }
    );
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

    assert_eq!(symbols[0].name, "qux");
    assert_eq!(
        symbols[0].info,
        ItemInfo::S4Method {
            signature: "Person, Other".into()
        }
    );
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

    assert_eq!(symbols[0].name, "foo");
    assert_eq!(
        symbols[0].info,
        ItemInfo::S4Method {
            signature: "Person, Other".into()
        }
    );
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

    let assert = |members: &[Item], name: &str, info: &ItemInfo| {
        assert!(
            members
                .iter()
                .any(|member| member.name == name && &member.info == info),
        );
    };

    assert_eq!(symbols.len(), 2);

    // Person
    assert_eq!(symbols[0].name, "Person");
    assert_eq!(symbols[0].info, ItemInfo::R6Class);

    let members = symbols[0].children.as_ref().unwrap();

    // Public properties and methods
    assert(members, "name", &ItemInfo::R6Field);
    assert(members, "age", &ItemInfo::R6Field);
    assert(members, "initialize", &ItemInfo::R6Method);
    assert(members, "greet", &ItemInfo::R6Method);
    assert(members, "say_age", &ItemInfo::R6Method);
    assert(members, ".hidden", &ItemInfo::R6Field);

    // Private properties and methods
    assert(members, "secret", &ItemInfo::R6Field);
    assert(members, "password", &ItemInfo::R6Field);
    assert(members, "reveal_secret", &ItemInfo::R6Method);

    // Active bindings
    assert(members, "full_name", &ItemInfo::R6Field);

    // Car
    assert_eq!(symbols[1].name, "Car");
    assert_eq!(symbols[1].info, ItemInfo::R6Class);

    let members = symbols[1].children.as_ref().unwrap();
    assert(members, "length", &ItemInfo::R6Field);
    assert(members, "drive", &ItemInfo::R6Method);
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
