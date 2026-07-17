//! Every token and node kind in the lossless R syntax tree.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
#[allow(non_camel_case_types)]
pub enum SyntaxKind {
    // ---- Tokens: trivia ----
    WHITESPACE,
    NEWLINE,
    COMMENT,
    /// The `#:` marker opening an annotation line; the annotation body is lexed
    /// into ordinary tokens and parsed into `TYPE_*` / directive nodes.
    ANNOTATION_MARKER,

    // ---- Tokens: literals ----
    INTEGER,
    DOUBLE,
    COMPLEX,
    STRING,
    RAW_STRING,

    // ---- Tokens: names ----
    IDENT,
    /// `...`
    DOTS,
    /// `..1`, `..2`, …
    DOTDOTI,
    /// `_` (the native-pipe placeholder)
    UNDERSCORE,

    // ---- Tokens: keywords ----
    FUNCTION_KW,
    IF_KW,
    ELSE_KW,
    REPEAT_KW,
    WHILE_KW,
    FOR_KW,
    IN_KW,
    NEXT_KW,
    BREAK_KW,
    TRUE_KW,
    FALSE_KW,
    NULL_KW,
    INF_KW,
    NAN_KW,
    NA_KW,
    NA_INTEGER_KW,
    NA_REAL_KW,
    NA_COMPLEX_KW,
    NA_CHARACTER_KW,

    // ---- Tokens: operators ----
    PLUS,
    MINUS,
    STAR,
    SLASH,
    CARET,
    /// Any `%…%` operator: `%%`, `%/%`, `%*%`, `%o%`, `%in%`, user `%op%`.
    SPECIAL,
    /// `|>`
    PIPE_GREATER,
    LESS,
    GREATER,
    LESS_EQ,
    GREATER_EQ,
    EQ2,
    BANG_EQ,
    BANG,
    AMP,
    AMP2,
    PIPE,
    PIPE2,
    TILDE,
    QUESTION,
    /// `<-`
    LESS_MINUS,
    /// `<<-`
    LESS2_MINUS,
    /// `->`
    MINUS_GREATER,
    /// `->>`
    MINUS_GREATER2,
    /// `=`
    EQ,
    /// `:=` (lexed by R as an operator; data.table gives it meaning)
    COLON_EQ,
    COLON,
    COLON2,
    COLON3,
    DOLLAR,
    AT,

    // ---- Tokens: punctuation ----
    L_PAREN,
    R_PAREN,
    L_BRACE,
    R_BRACE,
    L_BRACKET,
    /// `[[` (its close is two separate `]` tokens, as in R's own grammar)
    L_BRACKET2,
    R_BRACKET,
    COMMA,
    SEMICOLON,
    /// `\` (lambda shorthand `\(x) …`)
    BACKSLASH,

    /// A byte sequence the lexer cannot form a token from.
    ERROR_TOKEN,

    // ---- Nodes: expressions ----
    SOURCE_FILE,
    /// An identifier used as an expression (or assignment target).
    NAME,
    LITERAL,
    BINARY_EXPR,
    UNARY_EXPR,
    PAREN_EXPR,
    BRACE_EXPR,
    CALL_EXPR,
    ARGUMENT_LIST,
    ARGUMENT,
    /// `x[…]`
    SUBSET_EXPR,
    /// `x[[…]]`
    SUBSET2_EXPR,
    /// `x$field`
    DOLLAR_EXPR,
    /// `x@slot`
    AT_EXPR,
    /// `pkg::name` / `pkg:::name`
    NAMESPACE_EXPR,
    /// `function(…) body` and `\(…) body`
    FUNCTION_DEF,
    PARAMETER_LIST,
    PARAMETER,
    IF_EXPR,
    FOR_EXPR,
    WHILE_EXPR,
    REPEAT_EXPR,
    BREAK_EXPR,
    NEXT_EXPR,

    // ---- Nodes: `#:` annotations ----
    /// One annotation region: consecutive `#:` lines stitched into a single node.
    ANNOTATION,
    /// `@type` / `@alias` / `@param` / `@return` / `@forall` / `@strict` / `@trust` …
    ANNOTATION_DIRECTIVE,
    /// A bare or dotted type name.
    TYPE_REF,
    /// `list[T]`
    TYPE_APPLY,
    /// `T[]`
    TYPE_VECTOR,
    /// `A | B | NULL`
    TYPE_UNION,
    /// `fn(x: T) -> U`
    TYPE_FUNCTION,
    TYPE_PARAMETER_LIST,
    TYPE_PARAMETER,
    /// `<T, U: numeric>`
    TYPE_BINDER_LIST,
    TYPE_BINDER,
    /// `{a: integer, b: character}`
    TYPE_RECORD,
    TYPE_FIELD,
    TYPE_PAREN,
    TYPE_ARG_LIST,

    /// Recovery node: covers a region the parser could not form a construct from.
    ERROR,
}

use SyntaxKind::*;

impl SyntaxKind {
    pub fn is_trivia(self) -> bool {
        matches!(self, WHITESPACE | NEWLINE | COMMENT)
    }

    pub fn is_keyword(self) -> bool {
        matches!(
            self,
            FUNCTION_KW
                | IF_KW
                | ELSE_KW
                | REPEAT_KW
                | WHILE_KW
                | FOR_KW
                | IN_KW
                | NEXT_KW
                | BREAK_KW
                | TRUE_KW
                | FALSE_KW
                | NULL_KW
                | INF_KW
                | NAN_KW
                | NA_KW
                | NA_INTEGER_KW
                | NA_REAL_KW
                | NA_COMPLEX_KW
                | NA_CHARACTER_KW
        )
    }

    /// Keyword kind for a bare word, if reserved (a backticked spelling is never a keyword).
    pub fn from_keyword(word: &str) -> Option<SyntaxKind> {
        let kind = match word {
            "function" => FUNCTION_KW,
            "if" => IF_KW,
            "else" => ELSE_KW,
            "repeat" => REPEAT_KW,
            "while" => WHILE_KW,
            "for" => FOR_KW,
            "in" => IN_KW,
            "next" => NEXT_KW,
            "break" => BREAK_KW,
            "TRUE" => TRUE_KW,
            "FALSE" => FALSE_KW,
            "NULL" => NULL_KW,
            "Inf" => INF_KW,
            "NaN" => NAN_KW,
            "NA" => NA_KW,
            "NA_integer_" => NA_INTEGER_KW,
            "NA_real_" => NA_REAL_KW,
            "NA_complex_" => NA_COMPLEX_KW,
            "NA_character_" => NA_CHARACTER_KW,
            _ => return None,
        };
        Some(kind)
    }

    /// Human-readable spelling used in error messages ("expected `)` or `,`").
    pub fn display(self) -> &'static str {
        match self {
            WHITESPACE => "whitespace",
            NEWLINE => "newline",
            COMMENT => "comment",
            ANNOTATION_MARKER => "`#:`",
            INTEGER => "integer literal",
            DOUBLE => "number",
            COMPLEX => "complex literal",
            STRING => "string",
            RAW_STRING => "raw string",
            IDENT => "identifier",
            DOTS => "`...`",
            DOTDOTI => "`..N`",
            UNDERSCORE => "`_`",
            FUNCTION_KW => "`function`",
            IF_KW => "`if`",
            ELSE_KW => "`else`",
            REPEAT_KW => "`repeat`",
            WHILE_KW => "`while`",
            FOR_KW => "`for`",
            IN_KW => "`in`",
            NEXT_KW => "`next`",
            BREAK_KW => "`break`",
            TRUE_KW => "`TRUE`",
            FALSE_KW => "`FALSE`",
            NULL_KW => "`NULL`",
            INF_KW => "`Inf`",
            NAN_KW => "`NaN`",
            NA_KW => "`NA`",
            NA_INTEGER_KW => "`NA_integer_`",
            NA_REAL_KW => "`NA_real_`",
            NA_COMPLEX_KW => "`NA_complex_`",
            NA_CHARACTER_KW => "`NA_character_`",
            PLUS => "`+`",
            MINUS => "`-`",
            STAR => "`*`",
            SLASH => "`/`",
            CARET => "`^`",
            SPECIAL => "`%…%` operator",
            PIPE_GREATER => "`|>`",
            LESS => "`<`",
            GREATER => "`>`",
            LESS_EQ => "`<=`",
            GREATER_EQ => "`>=`",
            EQ2 => "`==`",
            BANG_EQ => "`!=`",
            BANG => "`!`",
            AMP => "`&`",
            AMP2 => "`&&`",
            PIPE => "`|`",
            PIPE2 => "`||`",
            TILDE => "`~`",
            QUESTION => "`?`",
            LESS_MINUS => "`<-`",
            LESS2_MINUS => "`<<-`",
            MINUS_GREATER => "`->`",
            MINUS_GREATER2 => "`->>`",
            EQ => "`=`",
            COLON_EQ => "`:=`",
            COLON => "`:`",
            COLON2 => "`::`",
            COLON3 => "`:::`",
            DOLLAR => "`$`",
            AT => "`@`",
            L_PAREN => "`(`",
            R_PAREN => "`)`",
            L_BRACE => "`{`",
            R_BRACE => "`}`",
            L_BRACKET => "`[`",
            L_BRACKET2 => "`[[`",
            R_BRACKET => "`]`",
            COMMA => "`,`",
            SEMICOLON => "`;`",
            BACKSLASH => "`\\`",
            ERROR_TOKEN => "unrecognized text",
            SOURCE_FILE => "source file",
            NAME => "name",
            LITERAL => "literal",
            BINARY_EXPR => "binary expression",
            UNARY_EXPR => "unary expression",
            PAREN_EXPR => "parenthesized expression",
            BRACE_EXPR => "braced expression",
            CALL_EXPR => "call",
            ARGUMENT_LIST => "argument list",
            ARGUMENT => "argument",
            SUBSET_EXPR => "subset expression",
            SUBSET2_EXPR => "subset expression",
            DOLLAR_EXPR => "`$` extraction",
            AT_EXPR => "`@` extraction",
            NAMESPACE_EXPR => "namespace access",
            FUNCTION_DEF => "function definition",
            PARAMETER_LIST => "parameter list",
            PARAMETER => "parameter",
            IF_EXPR => "`if` expression",
            FOR_EXPR => "`for` loop",
            WHILE_EXPR => "`while` loop",
            REPEAT_EXPR => "`repeat` loop",
            BREAK_EXPR => "`break`",
            NEXT_EXPR => "`next`",
            ANNOTATION => "type annotation",
            ANNOTATION_DIRECTIVE => "annotation directive",
            TYPE_REF => "type",
            TYPE_APPLY => "generic type",
            TYPE_VECTOR => "vector type",
            TYPE_UNION => "union type",
            TYPE_FUNCTION => "function type",
            TYPE_PARAMETER_LIST => "type parameter list",
            TYPE_PARAMETER => "type parameter",
            TYPE_BINDER_LIST => "type binder list",
            TYPE_BINDER => "type binder",
            TYPE_RECORD => "record type",
            TYPE_FIELD => "record field",
            TYPE_PAREN => "parenthesized type",
            TYPE_ARG_LIST => "type argument list",
            ERROR => "error",
        }
    }
}

impl From<SyntaxKind> for rowan::SyntaxKind {
    fn from(kind: SyntaxKind) -> Self {
        rowan::SyntaxKind(kind as u16)
    }
}
