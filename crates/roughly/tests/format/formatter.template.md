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

## Philosophy

The formatter follows these key principles:

* **Non-invasive formatting**: The formatter only adds line breaks if expressions are already multi-line, and won't break one-liners unnecessarily. See [Why Non-Invasive?](#why-non-invasive) for more details.
* **Auto-bracing and hugging**: Automatically adds braces when needed for clarity and applies intelligent spacing. See [Auto-Bracing](#auto-bracing) and [Hugging Behavior](#hugging-behavior) sections.
* **Minimal configuration**: The formatter works out-of-the-box with sensible defaults, so you can use it without any setup.

### Why Non-Invasive?

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
# switch_consistency_example : format
# Consistent formatting across all switch arms:
result <- switch(method,
  "simple" = calculate_simple_stats(data),
  "complex" = perform_advanced_statistical_analysis_with_multiple_parameters(data, alpha = 0.05, method = "robust"),
  "default" = get_basic_summary(data)
)

# Rather than breaking only the longer arm:
# result <- switch(method,
#   "simple" = calculate_simple_stats(data),
#   "complex" = perform_advanced_statistical_analysis_with_multiple_parameters(
#     data, alpha = 0.05, method = "robust"
#   ),
#   "default" = get_basic_summary(data)
# )
```

## Formatting Rules

Below is a comprehensive list of rules describing the behaviour of the formatter for different kinds of expressions including edge cases where special handling or nuanced behaviour is applied.

### Operators

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

### Blocks

Code blocks receive smart formatting based on their content and structure. Single-line blocks remain compact, while multi-line blocks format each expression on its own line:

```r
# braced_expression : compare
{x<-1;print(x)}
{
x<-1; print(x)
}
```

**Empty blocks** are consistently formatted:

```r
# braced_expression_empty : compare
{  }
```

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

### Line Spacing

The formatter normalizes line spacing between expressions, allowing at most one empty line:

```r
# line_spacing : compare
x <- 1
y <- 2


z <- 3
```

### Function Calls

Function calls receive consistent formatting with proper spacing around argument separators and assignment operators:

```r
# function_calls : compare
process(data=dataset,method="mean",na.rm=TRUE)
call(arg1,
    arg2=value,
        arg3)
```

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

**Mixed line formats** are allowed when the last argument starts on the same line:

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

setMethod("show", "MyClass", function(object) {
  cat("MyClass object\n")
})
```


### Function Definitions

Function definitions follow consistent formatting rules for parameters and body structure:

```r
# function_definitions : compare
process<-function(x,y=1){x+y}
filter<-function(data,method="simple"){
  select(data,method)
}
```

**Single line functions**: When the function body is a single expression, braces can be omitted if the expression starts on the same line. **Multi-line functions**: Always receive braces, even when the body starts on the same line as the function declaration.

**Anonymous functions** (lambda expressions) are also properly formatted:

```r
# anonymous_functions: compare
lapply(data,\(x)x+1)
```

**Body formatting**: If a function definition spans multiple lines or has a multiline condition, non-braced bodies are automatically wrapped in braces for consistency.

### Conditional Statements

**If statements** maintain compact formatting for simple conditions while ensuring readability for complex ones:

```r
# conditional_statements : compare
if(condition){action()} else{action()}

if(
  condition ||
  other_condition){
  action()
}
```

**Block enforcement**: If an if-statement has a multiline condition, the formatter ensures the body is wrapped in braces even if it's a single expression.

### Loops and Control Flow

**For loops** always enforce braced blocks for the body, ensuring consistency:

```r
# for_loops : compare
for(item in sequence) process(item)
```

**While loops** follow similar block enforcement rules:

```r
# while_loops : compare
while(condition) process()
```

**Repeat loops** also enforce braced blocks:

```r
# repeat_loops : compare
repeat process()
```

**Loop condition formatting**: Complex conditions that span multiple lines receive proper indentation within the parentheses.

### Parenthesized Expressions

Parenthesized expressions receive proper spacing for operators:

```r
# parenthesized_expressions : compare
(
  expression +
    other_part)
```

### String Literals

String literals receive intelligent quote normalization. The formatter prefers double quotes (`"`) unless the string contains unescaped double quotes:

```r
# string_literals : compare
message <- 'Hello world'
quoted_content <- 'Say "hello"'
```

### Subsetting and Member Access

**Bracket subsetting** follows the same formatting rules as function calls:

```r
# subsetting : compare
data[row,col]
data[["name"]]
object$value
```

**Namespace operators** (`::` and `:::`):

```r
# namespace_operators : compare
pkg::process
pkg:::filter
```

### Unary Operators

Unary operators receive appropriate spacing based on their type and context:

```r
# unary_operators : compare
result = ! condition
value = - 42
formula = ~ x + y
```

**Special spacing rule**: The `~` (formula) operator gets a space when followed by complex expressions, but not when followed by simple identifiers.

### Nested Block Expressions

When code blocks appear inside function calls or parenthesized expressions, the formatter applies smart indentation to avoid excessive nesting:

```r
# nested_block_expressions : compare
apply(data,1,function(row){
if(condition(row)){
transform(row)
}
})
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

### Special Language Constructs

- **Switch statements**: Fallthrough cases (`case = ,`) are handled correctly
- **Multi-line strings**: String literal structure is preserved

## Auto-Bracing

The formatter automatically adds braces to control flow structures when they improve clarity and consistency:

**Function definitions**: Multi-line functions always receive braces, even when the body starts on the same line:

```r
# auto_bracing_function_defintions : compare
process <- function(x) 
  x + 1
```

**Conditional statements**: Multi-line conditions or bodies are automatically braced:

```r
# auto_bracing_conditional_statements : compare
if (condition)
  action()
```

**Loops**: All loop bodies are automatically braced for consistency:

```r
# auto_bracing_loops : compare
for (i in 1:n)
  process(i)
```

## Hugging Behavior

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

**Non-invasive multi-line formatting**: When expressions are already multi-line, roughly only adds necessary spacing but preserves the overall structure. However, if all arguments don't fit on their separate lines, they will be properly separated:

```r
# non_invasive_multiline : compare
call(
  a=x,
  b=y, c=z)
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


## Line Endings

The formatter automatically detects and preserves the line ending style (`LF` or `CRLF`) used in the original file.

## Configuration

For details on configuring the formatter, see the [Configuration](/configuration) page.
