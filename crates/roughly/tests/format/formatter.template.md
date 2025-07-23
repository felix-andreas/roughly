---
title: Formatter
description: Documentation for Roughly's R code formatter.
---
<!-- R CODE IN THIS FILE IS FORMATTED AND SAVED TO docs/content/formatter.md -->

Roughly includes a non-invasive R code formatter that emphasizes readability while respecting the existing structure of your code.

## Usage

Format your R files using the command line:

```sh
roughly fmt          # Format all files in the current directory
roughly fmt <path>   # Format all files in <path>
roughly fmt --check  # Only check if files would be formatted
roughly fmt --diff   # Show diff of formatting changes without applying them
```

## Configuration

For details on configuring the formatter, see the [Configuration](/configuration) page.

## Philosophy

The formatter follows these key principles:

* **Non-invasive formatting**: The formatter only adds line breaks if expressions are already multi-line, and won't break one-liners unnecessarily. See [Why Non-Invasive?](#why-non-invasive) for more details.
* **Auto-bracing and hugging**: Automatically adds braces when needed for clarity and applies intelligent spacing. See [Auto-Bracing](#auto-bracing) and [Hugging Behavior](#hugging-behavior) sections.
* **Minimal configuration**: The formatter works out-of-the-box with sensible defaults, so you can use it without any setup.

## Formatting Rules

Below is a comprehensive list of rules describing the behaviour of the formatter for different kinds of expressions including edge cases where special handling is applied.

### Binary Operators

**Assignment operators** always get spaces around them:

```r
# assignment_operators : compare
x<-1
data<<-compute()
```

**Binary operators** get spaces around them, except for range (`:`) and power (`^`) operators:

```r
# binary_operators : compare
result=x+y*z
power=base^exponent
sequence=1:10
```

**Pipeline operators** maintain proper indentation when expressions span multiple lines:

```r
# pipeline_operators : compare
data %>%
filter(condition) %>%
select(value)
```

### Unary Operators

Unary operators receive appropriate spacing based on their type and context:

```r
# unary_operators : compare
result = ! condition
value = - 42
formula = ~x + y
```

**Special spacing rule**: The `~` (formula) operator gets a space when followed by complex expressions, but not when followed by simple identifiers.

### Blocks

**Multiline blocks** always have a newline after the opening brace and before the closing brace:

```r
# braced_blocks_multiline : compare
{ x <- 1
  print(x)
}
```

**Single-line blocks** are allowed, including those with semicolons. The formatter adds a space after `{` and before `}` for readability:

```r
# braced_blocks_single_line : compare
{x <- 1; print(x)}
```

**Semicolons in multiline blocks** are split into separate lines for clarity:

```r
# braced_blocks_semicolons_multiline : compare
{
  x <- 1; print(x)
}
```

**Empty blocks** have no space between the braces:

```r
# braced_expression_empty : compare
{  }
```

### Parenthesized Expressions

Single-line parenthesized expressions are always formatted in a "hugging" style—there is no extra space between the opening parenthesis and the enclosed expression:

```r
# parenthesized_expressions : compare
( x + y )
```

Multiline parenthesized expressions can be formatted in either hugged or expanded style; the formatter preserves both.

```r
# parenthesized_expressions_multiline : format
# expanded
(
  expression +
    other_part
)

# hugged
(expression +
    other_part)
```

### If expressions

**Single-line if-else**: Single-line `if-else` expressions are allowed and preserved, since `if` is an expression in R and can be used as a ternary operator:

```r
# conditional_statements_single_line : format
x <-if (condition) consequence else alternative
```

**Nested if-else**: Nested `if-else` chains are formatted so each `else if` and `else` starts on its own line, with all branches aligned at the same indentation level—no extra indentation for nested cases.

```r
# conditional_statements_nested : format
if (a) {
  x
} else if (b) {
  y
} else {
  z
}
```

**Auto-Bracing for multiline if-else**: Whenever an `if-else` spans multiple lines, all branches are always wrapped in braces for clarity and consistency:

```r
# conditional_statements_multiline : compare
if (condition) {
  consequence
} else alternative
```

**Auto-Bracing for multiline conditions**: If an `if` expression has a multiline condition, the formatter ensures the body is wrapped in braces even if it's a single expression:

```r
# if_statement_multiline_condition : compare
if (
  a && b
) body
```

### Loops

Loops are the only construct that are **not allowed** on a single line.

Because `for`, `while`, and `repeat` loops are used exclusively for their side effects and do not produce meaningful values, the formatter enforces that these statements are always written on multiple lines with explicit braces (see [auto-bracing](#auto-bracing)). This approach makes side effects visually clear and distinguishes loops from regular expressions.

**For loops** always enforce braced blocks for the body, ensuring consistency:

```r
# for_loops : compare
for(item in sequence) run_effect(item)
```

**While loops** follow similar block enforcement rules:

```r
# while_loops : compare
while(condition) action()
```

**Repeat loops** also enforce braced blocks:

```r
# repeat_loops : compare
repeat action()
```

**Multiline for loop headers**: The formatter allows both of the following styles for multiline `for` loop headers, preserving your preferred structure:

```r
# for_loops_multline_head: format
for (
    item in sequence
) {}

for (
    item
    in sequence
) {}
```


### Function Calls

Function calls receive consistent formatting with proper spacing around argument separators and assignment operators.

```r
# function_calls : compare
call(a,b=1,...)
```
**Multiline function calls**: Once two arguments appear on different lines, the call is treated as multiline, and each argument is formatted on its own line for clarity.

```r
# function_calls_multiline : compare
call(
  a = x,
  b = y, c = z
)
```

**Nested function calls** can use either a hugged style—where the inner call starts right after the outer call's parenthesis—or an expanded style. Both are preserved by the formatter, letting you choose the most readable form for your code.

```r
# hugging_nested_function_calls : format
# Hugged format - both functions start on the same line
result <- outer(inner(
  arg
))

# Expanded format - also valid
result <- outer(
  inner(
    arg
  )
)
```

**Trailing argument hugging** is allowed when the last argument starts on the same line: this means the final argument of a function call can begin immediately after the opening parenthesis or previous argument, even if it itself is multiline.

```r
# mixed_line_format : format
# This format is preserved - last argument starts on same line
call(a = x, b = y, c = inner(
  expr
))
```

This behavior is particularly useful for testing frameworks and S4 method definitions:

```r
# test_that_and_s4_example: format
test_that("description", {
  expect_equal(result, expected)
})

setMethod("method", "Class", function(x) {
  # ... implementation
})
```

### Function Definitions

**Single-line functions**: Functions with a simple, single-expression body can be written on one line, with or without braces.

```r
# function_definitions_single_line : format
add <- function(x, y) x + y
double <- function(x) { x * 2 }
```

**Multiline functions**: If the function body spans multiple lines, braces are always added—even if the body starts on the same line as the function declaration.

```r
# function_definitions_multiline : compare
function()
    call(a = x, b = y)
```

**Exception – multiline call on same line**: If the function body is a function call that starts on the same line and is itself multiline, braces are not required.

```r
# function_definitions_multiline_call : format
fn <- function() call(
    a = x,
    b = y
  )
```

**Anonymous functions (lambda expressions)**: Anonymous functions using `\` are formatted the same way as named functions, supporting both single-line and multiline bodies.

```r
# anonymous_functions_lambda : format
lapply(data, \(x) x + 1)

lapply(data, \(x) {
  y <- x * 2
  y + 1
})
```

### Switch Statements

Switch statements are formatted like normal function calls. For fallthrough cases (e.g., `case = ,`), an extra space is added after the `=` to clearly indicate the fallthrough.

```r
# switch_statements : format
result <- switch(
  type,
  "a" = handle_a(),
  "b" = ,
  "c" = handle_bc(),
  "default" = handle_default()
)
```

### Subsetting

**Bracket subsetting** follows the same formatting rules as function calls:

```r
# subsetting : compare
data[ row,col ]
data[[ "name" ]]
```

### Extract & Namespace operators

**Extract and namespace operators** (`$`, `@`, `::`, `:::`) are formatted without spaces around them:

```r
# extract_operator : compare
collection $ item
collection @ item
pkg :: process
pkg ::: filter
```

When chaining extract or namespace operators across multiple lines, the formatter indents each subsequent line to make the chain visually distinct and easy to follow:

```r
# extract_operator_chained : compare
object$
call(x)$
call(x, y)
```

### String Literals

String literals receive intelligent quote normalization. The formatter prefers double quotes (`"`) unless the string contains unescaped double quotes:

```r
# string_literals : compare
message <- 'Hello world'
quoted_content <- 'Say "hello"'
```

Multi-line string literals always keep their original indentation and line breaks, no matter where they appear. Even if surrounding code is refactored or deleted, the formatter never changes the internal content of multi-line strings.

```r
# multiline_strings : compare
# { <- parent block gets deleted
    x <- "This is a multi-line string.
          It preserves
          indentation and line breaks."
# }
```

### R6 Class Definitions

Class definitions with empty lines between methods are preserved:

```r
# r6_class_definitions : format
PersonClass <- R6Class(
  "Person",
  public = list(
    initialize = function(name) {
      private$name <- name
    },

    get_name = function() {
      return(private$name)
    }
  )
)
```

### Line Spacing

The formatter normalizes line spacing between expressions, allowing at most one empty line:

```r
# line_spacing : compare
x <- 1
y <- 2


z <- 3
```

### Line Endings

The formatter automatically detects and preserves the line ending style (`LF` or `CRLF`) used in the original file.


### Comments

In most cases, a space is added between the `#` and the comment text. For special comment types such as Roxygen (`#'`) and plumber (`#*`) comments, the space is inserted after the second character:

```r
# comments : compare
# comment with space
#comment without space
#'roxygen comment
#*plumber comment
#'string' <- commented out string
#!/usr/bin/env Rscript
```

Exceptions to this rule include:

- Commented-out strings such as `#'string'` are left unchanged, since inserting a space (e.g., `#' string'`) would alter the content.
- [Shebangs](https://en.wikipedia.org/wiki/Shebang_(Unix)), for example `#!/usr/bin/env Rscript`, remain unchanged.



## Auto-Bracing

The formatter always adds braces to control flow structures like `for`, `while`, and `repeat` loops.

**If expression**: Multi-line conditions or bodies are automatically braced:

```r
# auto_bracing_conditional_statements : compare
if (condition)
  action()
```

**Function definitions**: Multi-line functions always receive braces, even when the body starts on the same line:

```r
# auto_bracing_function_defintions : compare
process <- function(x)
  x + 1
```

This is done for two reasons:

**2. Accidental bugs:** It's easy to accidentally introduce subtle bugs when omitting braces in function definitions or `if` expressions. For example, if you later add a line after an unbraced `if`, only the first line is controlled by the condition:

```r
# _ : skip
# unbraced condition
if (condition)
  line1
  line2 # <- is meant to be in body

# how it it is interpreted:
if (condition)
  line1
line2 # <- get's executed unconditionally
```

## Hugging Behavior

When code blocks appear inside function calls or parenthesized expressions, the formatter applies smart indentation to avoid excessive nesting:

```r
# nested_block_expressions : compare
apply(data,1,function(row){
if(condition(row)){
transform(row)
}
})
```

"Hugging" refers to how nested expressions are formatted in multiline contexts - keeping them compact by allowing inner expressions to start on the same line as the outer expression's opening delimiter. This is part of roughly's non-invasive approach: both hugged and expanded formats are allowed, but hugging only applies to multiline expressions.

**Nested function calls** can be formatted in a hugged style:

```r
# hugging_nested_function_calls : format
# Hugged format - both functions start on the same line
result <- outer(inner(
  arg
))

# Expanded format - also valid
result <- outer(
  inner(
    arg
  )
)
```

**Parenthesized expressions** can also use hugging:

```r
# hugging_parenthesized : format
(expression +
  other_part)

# Also allowed
(
  expression +
    other_part
)
```

## Format Suppression

You can disable formatting for specific code sections using the `# fmt: skip` comment directive. This is useful when you want to preserve specific formatting for readability, such as aligned data structures.

```r
# format_suppression : format
# fmt: skip
matrix(
  c(
    1, 2,
    3, 4
  ),
  nrow=2
) # This code won't be reformatted

# fmt: skip
matrix(c(1, 2,
         3, 4), nrow=2) # The line above won't be reformatted
```

Without the `fmt: skip` directive, the `matrix(...)` expression would be broken into multiple lines according to standard formatting rules.

The `fmt: skip` directive can be placed:
- Before a line to skip formatting that entire expression
- At the end of a line to skip formatting just that line

You can also skip formatting for an entire file by placing `# fmt: skip-file` at the top of the file. This directive must be placed at the very beginning of the file to take effect.

## Why Non-Invasive?

The non-invasive approach means Roughly respects your existing line breaks and won't arbitrarily split expressions that you've chosen to keep on one line. **Non-invasive formatting tries to minimize the amount of line-breaks not set by the programmer** by following these rules:

- **Single line expressions are never broken into multiple lines** (with the exception of loops like `for`, `while`, `repeat`, because they don't yield useful values and can only perform side effects, so they are not normal expressions in that sense)
- **Both hugging and not hugging is allowed** for function calls and other constructs. See [Auto-Bracing](#auto-bracing) and [Hugging Behavior](#hugging-behavior) sections.
- **Preserves programmer intent** regarding line structure and formatting choices

**The trade-off between readability from line breaks versus long lines should be in the hands of the author**, as this trade-off depends heavily on context. For numerical expressions, long lines are often more preferable because they preserve the mathematical structure and relationships that would be obscured by arbitrary line breaking.


#### Why This Matters for Numerical Computing

R is a numerical language, and numerical expressions tend to get ugly when broken up by line length limits. Consider this mathematical expression:

```r
# non_invasive_numerical_example : format
# Without non-invasive formatting, this compact expression:
n <- 1 + (length(replacement_id) - 1) * (vectorToSearch == valueTypeToReplace)

# Might get broken up into this less readable form:
# n <- 1 +
#   (length(replacement_id) - 1) *
#     (vectorToSearch == valueTypeToReplace)
```

This forced line breaking can lead to several problems:
- People might use shorter, less descriptive variable names just to fit expressions on one line
- Intermediate results might be stored in variables that have no meaningful purpose or don't correspond to established mathematical formulas
- The mathematical relationship becomes harder to understand

#### Consistency Between Related Lines

Sometimes you want consistency between multiple consecutive lines, especially when they follow the same pattern but have slight variations. For example, if you have two lines that only differ by a prefix or suffix, it's better to keep them both on single lines for easy comparison:

```r
# consistency_example : format
# This is easier to read and compare:
start <- camera_origin_start - (focus_dist * forward_start) - 0.5 * view_width * right_start + 0.5 * view_height * up_start
end <- camera_origin_end - (focus_dist * forward_end) - 0.5 * view_width * right_end + 0.5 * view_height * up_end

# Than this inconsistent formatting where only one line is broken:
# start <- camera_origin_start - (focus_dist * forward_start) - 0.5 * view_width * right_start
#     + 0.5 * view_height * up_start
# end <- camera_origin_end - (focus_dist * forward_end) - 0.5 * view_width * right_end + 0.5 * view_height * up_end
```

#### Switch Statement Example

The same principle applies to switch statements where one arm might have a longer expression. Non-invasive formatting keeps consistency across all arms:

```r
# _ : skip
# Consistent formatting across all switch arms:
result <- switch(method,
  "simple" = calculate_simple_stats(data),
  "complex" = perform_advanced_statistical_analysis_with_multiple_parameters(data, alpha = 0.05, method = "robust"),
  "default" = get_basic_summary(data)
)

# Rather than breaking only the longer arm:
result <- switch(method,
  "simple" = calculate_simple_stats(data),
  "complex" = perform_advanced_statistical_analysis_with_multiple_parameters(
    data, alpha = 0.05, method = "robust"
  ),
  "default" = get_basic_summary(data)
)
```

