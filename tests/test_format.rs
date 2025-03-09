use {
    indoc::indoc,
    ropey::Rope,
    roughly::{
        format::{Config, FormatError, format},
        tree,
    },
};

macro_rules! assert_fmt {
    ($input:expr) => {
        insta::assert_snapshot!(format_str(indoc! {$input}).unwrap());
    };
}

fn format_str(text: &str) -> Result<String, FormatError> {
    let tree = tree::parse(text, None);

    // DEBUG
    // dbg!(tree.root_node().to_sexp());
    // eprintln!("{}", utils::format_node(&tree.root_node()));
    format(tree.root_node(), &Rope::from_str(text), Config {
        stop_on_unhandled_comment: false,
        spaces: 2,
    })
}

#[test]
fn binary_operator() {
    assert_fmt! {r#"
        4 + 2
        4 + 2*3
        4 +
            3 +
                2 +
                    1
    "#};
    assert_fmt! {r#"
        1:10
    "#}
    // assignments
    assert_fmt! {r#"
        x<-1
    "#};
    assert_fmt! {r#"
        x<-1;y<-2
    "#};
    // pipeline operator
    assert_fmt! {r#"
        foo |>
            bar
    "#};
    assert_fmt! {r#"
        foo %>% bar %>%
                baz
    "#};
    assert_fmt! {r#"
        foo |> # foo
        #bar
        bar |>#bar
        baz |>
            qux
    "#};
    assert_fmt! {r#"
        foo |>
            # something about bar
            bar |>
            # something about baz
            baz
        foo |># 1
            # 2
                bar |> # 3
            # 4
            # 5
            # 6
                baz
    "#};
    // binary expression where one side is multilien
    assert_fmt! {r#"
        foo(
            1
        ) + 2
        (
            1
        ) +
        2
        1 + foo(
        2)
        1 +
        foo(
        2)
    "#};
}

#[test]
fn braced_expression() {
    assert_fmt! {r#"
        {}
        {
        }
    "#};
    assert_fmt! {r#"
        { 1L;2}
    "#};
    assert_fmt! {r#"
        {
            foo
            bar
        }
    "#};
    assert_fmt! {r#"
        {foo;
            bar}
    "#};
    assert_fmt! {r#"
        {
            a # foo
        }
    "#};
    assert_fmt! {r#"
        # foo
        { # bar
        #baz
        a
        # qux
        }
    "#};
    assert_fmt! {r#"
        {
            # single line
            a # next


            # multi
            # line
            # comment
            b
        }
    "#};
    assert_fmt! {r#"
        {
            a

            b
        }
    "#};
    assert_fmt! {r#"
        { foo; bar }
    "#};
}

#[test]
fn call() {
    assert_fmt! {r#"
        list  (a = 1, b= 2L ,c =3i  )
    "#};
    assert_fmt! {r#"
        list  (a = 1,
         b= 2L)
    "#};
    assert_fmt! {r#"
        list  (
            # foo
            a = 1, #bar
            b= 2L) #baz
    "#};
    assert_fmt! {r#"
        foo  ( #foo
            # foo
            f
            # foo bar
            #   foo bar
            #   foo bar
            a = 1, #bar


            b= 2L) #baz

            # foo
    "#};
    assert_fmt! {r#"
        foo({ bar; baz })
        foo({ bar;
        baz })
        foo({ bar;
        baz }, qux)
        foo(qux = { bar;
        baz }, qux)
    "#};
}

#[test]
fn extract_operator() {
    assert_fmt! {r#"
        foo@bar
        foo$bar
        foo @ bar
        foo$  bar
    "#};
    assert_fmt! {r#"
        ( foo+ bar )@baz
    "#};
    assert_fmt! {r#"
        list(foo = 1, bar =
        2)@baz
    "#};

    // note: this is not parsed correctly
    // see: https://github.com/users/felix-andreas/projects/5/views/1?pane=issue&itemId=100962575
    // assert_fmt! {r#"
    //     foo$
    //
    //     bar
    // "#};
}

#[test]
fn for_statement() {
    assert_fmt! {r#"
        for (x in 1:2) {
            print(x)
        }
        for (x in c(1, # foo
        2)) {
            print(x)
        }
        for (x in 1:3) #foo
        {
            x
        }
        for (x in 1:3)
        #foo
        {
            x
        }
    "#};

    assert_fmt! {r#"
        for (x in foo(
        bar)) baz
    "#};

    assert_fmt! {r#"
        for (x in foo( bar)) { baz }
        for (x in
        foo( bar)) { baz }
    "#};

    // check allowed multiline combinations
    assert_fmt! {r#"
        for (variable in sequence) {}

        for (
            variable in sequence
        ) {}

        for (
            variable
            in sequence
        ) {}
    "#};

    // check all possible comment positions
    assert_fmt! {r#"
        for
        # 1
        ( # (
            # 2
            variable # variable
            # 3
            in
            # 4
            sequence # sequence
            # 5
        ) # )
        # 6
        {
            body
        }
    "#};

    // check multiline sequences in combination with comments
    assert_fmt! {r#"
        for (
        variable
        # 1
        in {
            sequence
        }) {}

        for (
        variable
        in
        # 2
        {
            sequence
        }) {}
        "#
    };

    // comment after condition but no block
    assert_fmt! {r#"
        for (variable in sequence) # 1
            body
        "#
    };
}

#[test]
fn function_definition() {
    assert_fmt! {r#"
        function(a, b= "foo") {}
    "#};
    assert_fmt! {r#"
        function(a,
        b=  "foo") {}
    "#};
    assert_fmt! {r#"
        (\(a, b) a *  b)(2, 3)
    "#};
    assert_fmt! {r#"
        function(
            a , b=  "foo") {}
    "#};
    assert_fmt! {r#"
        function(
        ) {}
    "#};
    assert_fmt! {r#"
        function(
            # foo
            foo, #foo
            #bar
            #  bar
            bar = 3 #bar
        ) {}
    "#};
    assert_fmt! {r#"
        function (a,
        b) baz
    "#};
    assert_fmt! {r#"
        function (a,
        b) {baz}
    "#};
}

#[test]
fn if_statement() {
    assert_fmt! {r#"
        if (a>b) {
            1
        } else {
        "foo"
        }
        x <- if (T) 4
        if (TRUE) #foo
        10

        if (foo <bar) {
        lala
        1} else if (1 >2) {2
        } else {3}
    "#};

    // multiline condition
    assert_fmt! {r#"
        if (any(
        sapply(foo)
        )) stop("")
        if ( !foo ||
        !bar ||
            !baz()
        ) stop("")
    "#};

    // if else if else
    assert_fmt! {r#"
        if (
            TRUE
        ) {foo
         } else if (TRUE) {bar
        } else baz
    "#};

    assert_fmt! {r#"
        if (foo) {bar}
        if (foo) {bar} else {baz}
        if (foo) bar else {baz}
        if (
        foo) {bar}
        if (
        foo) {bar}
        if (foo)
            {bar}
    "#};

    // make nested alternatives multiline
    assert_fmt! {r#"
        if (foo) {
            bar
        } else if (baz) { qux } else corge

        if (foo)
            {bar}
        if (foo) {
            bar
        } else if (baz) {
            qux
        } else if (quux) { corge }
    "#};

    // make_multiline is not transitiv (doesn't break single line if-else)
    assert_fmt! {r#"
        function()
            if (foo) bar else baz

        function() {
            if (foo) bar else baz
        }
    "#};

    // condition is braced expression
    assert_fmt! {r#"
        if ({ foo; bar }) { baz }
        if ({ foo;
         bar }) { baz }
    "#};
}

#[test]
fn if_statement_comments() {
    assert_fmt! {r#"
        # for some weird reason this is only parsed correctly if in a parentheses
        (
            # before
            if # if
            # 1
            ( # open
                # 2
                a && b # condition
                # 3
            ) # close
            # 4
            {
                y
            }
            # 5
            else # else
            # 6
            {
                4
            }
            # after
        )
    "#};

    // special case: wrap braced expression with newlines if condition contains comments
    assert_fmt! {r#"
        if (
        # 1
        {
            condition
        }) {}

        if ({
            condition
        }
        # 1
        ) {}
        "#
    };

    // comment after condition but no block
    assert_fmt! {r#"
        if (condition) # 1
            body
        "#
    };
}

#[test]
fn namespace_operator() {
    assert_fmt! {r#"
        foo::
        foo::bar
        foo::bar(1)
    "#}
}

#[test]
fn parenthesized_expression() {
    assert_fmt! {r#"
        (1 +2 )
        (
        #foo
        1 +2 )
        x <- ( # com
        5
        )
        (
            a #foo
        )
        (
            #foo
            a #bar
            #baz
        )
        ( #foo
            a #bar
        #baz
        )
        ( a #foo
        )
    "#};
}

#[test]
fn program() {
    assert_fmt! {r#"
        # A simple comment
        x <- 1 + 2
        y <- x * 3
        z <- if (y > 5) {
        "greater"
        } else {
        "lesser"
        }
        result <- function(a, b = 2) {
        return(a + b)
        }
        list <- list(a = 1, b = 2, c = 3)
        for (i in 1:10) {
        print(i)
        }
        while (x < 10) {
        x <- x + 1
        }
        repeat {
        x <- x - 1
        if (x == 0) break
        }
        foo <- function(x) x^2
        bar <- foo(3)
        baz <- c(1, 2, 3)
        qux <- baz[1]
        quux <- baz[[1]]
        corge <- list(a = 1, b = 2)
        grault <- corge$a
        garply <- corge[["b"]]
        waldo <- TRUE
        fred <- FALSE
        plugh <- NULL
        xyzzy <- Inf
        thud <- NaN
    "#};
}

#[test]
fn repeat_statement() {
    assert_fmt! {r#"
        repeat { body }
        repeat {
            body
        }
        repeat body
        repeat
            body
    "#};

    // check all possible comment positions
    assert_fmt! {r#"
        repeat # repeat
        # 1
        {
        }
    "#};

    // comment after condition but no block
    assert_fmt! {r#"
        repeat # 1
            body
        repeat # 1
        { body }
        "#
    };
}

#[test]
fn string() {
    assert_fmt! {r#"
        '"foo"'
        "\"foo\""
        '\"foo"'
        '\\"foo\\"'
        "\\\"foo\\\""
    "#};
    assert_fmt! {r#"
        "foo
            bar"
        foo("foo
            bar")
    "#};
}

#[test]
fn subset() {
    assert_fmt! {r#"
        foo[x]
        foo[x,   y]
        foo[1, 2 ]
        foo[x=1  , ,y  =3,4]
        foo[  ]
        foo[ , ]
        foo[, ,]
        foo[ x, ]
        foo[x,,]
        foo[,x ]
        foo[,,x]
        foo[x, ,y]
        foo[,,x,,y,,]
        foo[ #foo
        1,2,3
        ]
        foo[ #foo
        #bar
        1,2,3
        #baz
        ]
        foo[ #foo
        ,,a
        ]
    "#};
}

#[test]
fn subset2() {
    assert_fmt! {r#"
        foo[[x ]]
        foo[[x,   y]]
        foo[[1, 2 ]]
        foo[[x=1  , ,y  =3,4]]
        foo[[  ]]
        foo[[ , ]]
        foo[[, ,]]
        foo[[ x, ]]
        foo[[x,,]]
        foo[[,x ]]
        foo[[,,x]]
        foo[[x, ,y]]
        foo[[,,x,,y,,]]
        foo[[ #foo
        1,2,3
        ]]
        foo[[ #foo
        #bar
        1,2,3
        #baz
        ]]
        foo[[ #foo
        ,,a
        ]]
    "#};
}

#[test]
fn unary_operator() {
    assert_fmt! {r#"
        !a
        +a
        -a
        foo(!a, +   b)
        foo(- a , bar)
        !
        a
        -  b
        -42
        + 42
        !TRUE
        ~foo
        -foo + bar
        -  (foo + bar)
        ! foo && bar
        ~  foo | bar
    "#};
}

#[test]
fn while_statement() {
    assert_fmt! {r#"
        while(x < 10)
        { print(x)
            x <- x + 1
        }
        while (x < 10) { #foo
            print(x) }
        while (x < 10)
        #foo
        {
            print(x)
        }
    "#};

    assert_fmt! {r#"
        while(foo(
        bar)) baz
    "#};

    assert_fmt! {r#"
        while(foo(
        bar)) {baz}
    "#};

    assert_fmt! {r#"
        while ({ foo; bar }) { baz }
        while ({ foo;
        bar }) { baz }
    "#};

    // comments in multi-line condition
    assert_fmt! {r#"
        while(
        	# 1
            foo && # foo
                bar # bar
            # 2
        )
        {}
    "#};

    // check all possible comment positions
    assert_fmt! {r#"
        while # while
        # 1
        ( # (
            # 2
            condition # condition
            # 3
        ) # )
        # 4
        {}
    "#};

    // special case: wrap braced expression with newlines if condition contains comments
    assert_fmt! {r#"
        while (
        # 1
        {
            condition
        }) {}

        while ({
            condition
        }
        # 1
        ) {}
        "#
    };

    // comment after condition but no block
    assert_fmt! {r#"
        while (condition) # 1
            body
        "#
    };
}

// EDGE CASES
#[test]
fn comment_formatting() {
    assert_fmt! {r#"
        #foo
        ##foo
        ## foo
        ### foo
        # # foo
        #    foo
        #'foo
        #
        # #
        #'@param
        #' @param
        #"foo"
        #'foo'
        #'foo
    "#};

    assert_eq!(
        "#'\n#' foo\nx <- 1\n",
        format_str("#' \n#' foo\nx<-1").unwrap(),
    )
}

#[test]
fn chained_extract_operators() {
    assert_fmt! {r#"
        foo$
            bar(a)$
            baz(a,b)

        (foo
        )$
            bar(a
            )$
            baz(a,b)$
    "#};
}

#[test]
fn line_formatting() {
    assert_eq!(
        "foo\nbar\nbaz\n",
        format_str("foo \n bar \n baz \n").unwrap()
    );
    assert_eq!(
        "foo\nbar\nbaz\n",
        format_str("foo\nbar\r\nbaz\r\n").unwrap()
    );
    assert_eq!(
        "foo\r\nbar\r\nbaz\r\n",
        format_str("foo\r\nbar\r\nbaz\r\n").unwrap()
    );
    assert_eq!(
        "foo\r\nbar\r\nbaz\r\n",
        format_str("foo\r\nbar\nbaz\n").unwrap()
    );
}

#[test]
fn r6_allow_newlines() {
    assert_fmt! {r#"
        Person <- R6Class(
            "Person",
            public = list(
                initialize = function(name, age = NA) {
                    private$name <- name
                    private$age <- age
                },

                print = function(...) {
                    cat("Person: \n")
                    cat("  Name: ", private$name, "\n", sep = "")
                    cat("  Age:  ", private$age, "\n", sep = "")
                }
            ),

            private = list(
                age = NA,
                name = NULL
            )
        )
    "#};
}

#[test]
fn switch_fallthrough() {
    assert_fmt! {r#"
        switch(foo,
            x = 1,
            "y" = 2,
            z = ,
            3
        )
    "#};
}

#[test]
fn semicolons_in_function() {
    assert_fmt! {r#"
        function(x) {
            names(foo[[x]]) <- bar; foo[x]
        }
    "#};
}

// LIBRARIES WITH SPECIAL FORMATTING
#[test]
fn data_table() {
    assert_fmt! {r#"
        ans <- flights[, .(arr_delay, dep_delay)]
        DT[,.(V4.Sum=sum(V4)), by=V1][order(-V1)]
        DT[,':='(V1=round(exp(V1),2), V2=LETTERS[4:6])][]
        DT[,lapply(.SD,sum),by=V2, # comment
            .SDcols=c("V3","V4")]
    "#};
}

#[test]
fn dplyr() {
    assert_fmt! {r#"
        starwars %>% #foo
        group_by(species)   %>% #bar
        select(height, mass)%>% ###   baz
        summarise(
                height = mean(height, na.rm = TRUE),
            mass = mean(mass, na.rm = TRUE)
        )
    "#};
}

#[test]
fn purrr() {
    assert_fmt! {r#"
        library(purrr)

                mtcars |>
            split(mtcars$cyl) |>  # from base R
        map(\(df) lm(mpg~wt, data    = df)) |>
        map(summary  ) |>
        map_dbl("r.squared"  )
    "#};
}

// ERROR CASES
#[test]
fn error() {
    let result = format_str(indoc! {r#"
        function
    "#});

    let Err(FormatError::SyntaxError { kind, line, col }) = result else {
        panic!()
    };
    assert_eq!(kind, "ERROR");
    assert_eq!(line, 0);
    assert_eq!(col, 0);
}

#[test]
fn missing() {
    let result = format_str(indoc! {r#"
        x <- 1
        function() { # missing function body
            x <- 2
            x <- 3
        x <- 3
    "#});

    assert!(matches!(
        result,
        Err(FormatError::Missing {
            kind: "}",
            line: 5,
            col: 0
        })
    ));

    let result = format_str(indoc! {r#"
        foo(
    "#});
    assert!(matches!(
        result,
        Err(FormatError::Missing {
            kind: ")",
            line: 0,
            col: 4
        })
    ));
}

// DIRECTIVES
#[test]
fn fmt_skip() {
    assert_fmt! {r#"
        # fmt: skip
        foo <- c(1,2,
            3)
    "#};
    assert_fmt! {r#"
        foo <- c(1,2,
        3)# fmt: skip
        bar <- c(1,2,
        3)
    "#};
    assert_fmt! {r#"
        foo <- c(1,2,
        3)
        # fmt: skip
        bar <- c(1,2,
        3)
    "#};
    assert_fmt! {r#"
        {
            foo <- c(1,2,
            3)
            # fmt: skip
            bar <- c(1,2,
                3)
            foo <- c(1,2,
                3) # fmt: skip
            bar <- c(1,2,
            3)
        }
    "#};
    assert_fmt! {r#"
        foo(
          # fmt: skip
          a = c(
            1, 2,
            3, 4
          ),
          b=0
        )
        c(
          1, 2,
          3, 4
        ) # fmt: skip
    "#};
}

// FROM
// https://github.com/r-lib/tree-sitter-r/blob/main/test/corpus/literals.txt
// https://github.com/r-lib/tree-sitter-r/blob/main/test/corpus/expressions.txt
#[test]
fn comments() {
    assert_fmt! {r#"
        # a comment'

        '# not a comment'


        '
        # still not a comment'
    "#}

    assert_fmt! {r#"
        #!/usr/bin/env Rscript
    "#}
}

#[test]
fn constants() {
    assert_fmt! {r#"
        TRUE
        FALSE
        NULL
        Inf
        NaN
        NA
        NA_real_
        NA_character_
        NA_complex_
    "#}
}

#[test]
fn identifiers() {
    assert_fmt! {r#"
        foo
        foo2
        foo.bar
        .foo.bar
        .__NAMESPACE__.
        foo_bar
        `_foo`
        `a "literal"`
        `another
        literal \` foo`
        `backslash followed by newline \
        `
        `\``
        # Pipe placeholder
        _
        # Recognized as a single `_foo` identifier, even if invalid R code (#71).
        _foo
        __foo
        _foo_
    "#}
}

#[test]
fn strings() {
    assert_fmt! {r##"
        ""
        ''
        "foo"
        "foo
        bar"
        "#"
        ","
        "}"
        'foo'
        'foo
        bar'
        '#'
        ','
        '}'
    "##}
}

#[test]
fn integers() {
    assert_fmt! {r#"
        12332L
        0L
        12L
        0xDEADL
        1e1L
        # Technically, R parses this as a float with a warning, but for our purposes this is good enough
        0.1L
    "#}
}

#[test]
fn floats() {
    assert_fmt! {r#"
        .66
        .11
        123.4123
        .1234
        0xDEAD
        x <- -.66
        1e322
        1e-3
        1e+3
        1.8e10
        1.e10
        1e10
    "#}
}

#[test]
fn dot_dot_i() {
    assert_fmt! {r#"
        ..1
        ..10
    "#}
}
