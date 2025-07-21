---
title: Formatter
description: Documentation for Roughly's R code formatter.
---

Roughly includes a non-invasive R code formatter that emphasizes readability while respecting the existing structure of your code.

## Usage

Format your R files using the command line:

```sh
roughly fmt                # Format all files in the current directory
roughly fmt <path>         # Format all files in <path>
roughly fmt --check        # Only check if files would be formatted
roughly fmt --diff         # Show diff of formatting changes without applying them
```

## Philosophy

The formatter follows these key principles:

* **Non-invasive formatting**: The formatter only adds line breaks if expressions are already multi-line, and won't break one-liners unnecessarily. See [Why Non-Invasive?](#why-non-invasive) for more details.
* **Comment preservation**: Maintains all comments while improving their formatting
* **Smart indentation**: Uses context-aware indentation for complex expressions
* **Auto-bracing and hugging**: Automatically adds braces when needed for clarity and applies intelligent spacing. See [Auto-Bracing](#auto-bracing) and [Hugging Behavior](#hugging-behavior) sections.

### Why Non-Invasive?

R is an expression-based language with a strong focus on numerical computing and data analysis. Unlike many other programming languages, R code is often written interactively and exploratively, where preserving the original intent and structure of expressions is crucial for readability and debugging.

The non-invasive approach means roughly respects your existing line breaks and won't arbitrarily split expressions that you've chosen to keep on one line. This is particularly important in R because:

- **Data analysis workflows**: Short, expressive one-liners are common and meaningful
- **Interactive development**: Code is often built incrementally, and forced line breaks can disrupt the flow
- **Mathematical expressions**: Complex formulas are often more readable when kept compact
- **Functional style**: R's functional nature benefits from preserving the structure of nested calls

## Formatting Rules

Roughly applies specific formatting rules to different R code constructs. The formatter analyzes the abstract syntax tree to make intelligent decisions about spacing, line breaks, and indentation.


### Assignment and Operators

**Assignment operators** always get spaces around them:

```r
# Before formatting
x<-1
result<<-calculate()

# After formatting
x <- 1
result <<- calculate()
```

**Binary operators** get spaces around them, except for range (`:`) and power (`^`) operators:

```r
# Before formatting
result=x+y*z
power=base^exponent
sequence=1:10

# After formatting
result = x + y * z
power = base^exponent
sequence = 1:10
```

**Pipeline operators** maintain proper indentation when expressions span multiple lines:

```r
# Before formatting
data %>%
filter(condition) %>%
summarize(mean_value=mean(value))

# After formatting
data %>%
  filter(condition) %>%
  summarize(mean_value = mean(value))
```

### Code Blocks and Braced Expressions

Code blocks receive smart formatting based on their content and structure. Single-line blocks remain compact, while multi-line blocks format each expression on its own line:

```r
# Before formatting
{x<-1;print(x)}
{
x<-1; print(x)
}

# After formatting
{ x <- 1; print(x) }
{
  x <- 1
  print(x)
}
```

**Empty blocks** are consistently formatted:

```r
# Before formatting
{  }

# After formatting
{}
```

### Comments

Comments are reformatted to ensure consistent spacing while preserving their content and meaning:

```r
# Before formatting
#This is a comment
#'This is roxygen
##This is a header

# After formatting
# This is a comment
#' This is roxygen
##This is a header
```

**Special comment types** are preserved:

- Roxygen comments (`#'`) maintain their structure
- Header comments (`##`, `###`) keep their formatting
- Shebangs (`#!/usr/bin/env Rscript`) remain unchanged
- Comments with quotes (`#'quoted'`) are left as-is to avoid conflicts

### Line Spacing

The formatter normalizes line spacing between expressions, allowing at most one empty line:

```r
# Before formatting
calculate_mean <- function(data) {
  clean_data <- data[!is.na(data)]


  mean(clean_data)
}

# After formatting
calculate_mean <- function(data) {
  clean_data <- data[!is.na(data)]

  mean(clean_data)
}
```

### Function Calls and Arguments

Function calls receive consistent formatting with proper spacing around argument separators and assignment operators:

```r
# Before formatting
calculate(data=dataset,method="mean",na.rm=TRUE)
complex_call(argument1,
    argument2=value,
        argument3)

# After formatting
calculate(data = dataset, method = "mean", na.rm = TRUE)
complex_call(
  argument1,
  argument2 = value,
  argument3
)
```

**Argument hugging**: When function arguments can fit on a single line and the last argument's value starts on the same line as the opening parenthesis, the formatter keeps a compact format. Otherwise, it expands to multiple lines with proper indentation.

**Nested function calls** are formatted with hugging when appropriate:

```r
# Before formatting
result <- outer(inner(a,b),other(c,d))

# After formatting  
result <- outer(inner(a, b), other(c, d))
```

### Function Definitions

Function definitions follow consistent formatting rules for parameters and body structure:

```r
# Before formatting
calc<-function(x,y=1){x+y}
stats<-function(data,method="mean"){
  result(data,method)
}

# After formatting
calc <- function(x, y = 1) x + y
stats <- function(data, method = "mean") {
  result(data, method)
}
```

**Single line functions**: When the function body is a single expression, braces can be omitted if the expression starts on the same line. **Multi-line functions**: Always receive braces, even when the body starts on the same line as the function declaration.

**Anonymous functions** (lambda expressions) are also properly formatted:

```r
# Before formatting
lapply(data,\(x)x+1)

# After formatting
lapply(data, \(x) x + 1)
```

**Body formatting**: If a function definition spans multiple lines or has a multiline condition, non-braced bodies are automatically wrapped in braces for consistency.

### Conditional Statements

**If statements** maintain compact formatting for simple conditions while ensuring readability for complex ones:

```r
# Before formatting
if(condition){action} else{alternative}
if(long_condition ||
   other_condition){
  action()
}

# After formatting
if (condition) { action } else { alternative }
if (
  long_condition ||
  other_condition
) {
  action()
}
```

**Condition hugging**: When conditions fit on a single line without comments, the formatter keeps them compact. For multiline conditions, proper indentation is applied.

**Block enforcement**: If an if-statement has a multiline condition, the formatter ensures the body is wrapped in braces even if it's a single expression.

### Loops and Control Flow

**For loops** always enforce braced blocks for the body, ensuring consistency:

```r
# Before formatting
for(item in collection) process(item)

# After formatting
for (item in collection) {
  process(item)
}
```

**While loops** follow similar block enforcement rules:

```r
# Before formatting
while(condition) action()

# After formatting
while (condition) {
  action()
}
```

**Repeat loops** also enforce braced blocks:

```r
# Before formatting
repeat action()

# After formatting
repeat {
  action()
}
```

**Loop condition formatting**: Complex conditions that span multiple lines receive proper indentation within the parentheses.

### Parenthesized Expressions

Parenthesized expressions maintain their layout with smart formatting for readability:

```r
# Before formatting
(x+y*z)
(long_expression+
 other_part)

# After formatting
(x + y * z)
(
  long_expression +
  other_part
)
```

**Parenthesis hugging**: If the content fits on one line, parentheses hug the content. For multiline content, proper indentation is applied.

### String Literals

String literals receive intelligent quote normalization. The formatter prefers double quotes (`"`) unless the string contains unescaped double quotes:

```r
# Before formatting
message <- 'Hello world'
quoted_content <- 'Say "hello"'

# After formatting
message <- "Hello world"
quoted_content <- 'Say "hello"'
```

### Subsetting and Member Access

**Bracket subsetting** follows the same formatting rules as function calls:

```r
# Before formatting
data[row_index,column_index]
environment[["variable_name"]]
object$member_variable

# After formatting
data[row_index, column_index]
environment[["variable_name"]]
object$member_variable
```

**Namespace operators** (`::` and `:::`):

```r
# Before formatting
package::public_function
package:::private_function

# After formatting
package::public_function
package:::private_function
```

### Unary Operators

Unary operators receive appropriate spacing based on their type and context:

```r
# Before formatting
result = ! condition
number = - 42
formula = ~ response + predictor

# After formatting
result = !condition
number = -42
formula = ~ response + predictor
```

**Special spacing rule**: The `~` (formula) operator gets a space when followed by complex expressions, but not when followed by simple identifiers.

## Format Suppression

You can disable formatting for specific code sections using the `# fmt: skip` comment directive:

```r
# fmt: skip
matrix(
  c(
    1, 2,
    3, 4
  ),
  nrow=2
) # This code won't be reformatted

matrix(c(1, 2,
         3, 4), nrow = 2)  # This code will be formatted

matrix(c(1, 2,
         3, 4), nrow=2) # fmt: skip
# The line above won't be reformatted
```

The `fmt: skip` directive can be placed:
- Before a line to skip formatting that entire expression
- At the end of a line to skip formatting just that line

## Advanced Formatting Features

The formatter intelligently handles various R idioms and special patterns:

### Nested Block Expressions
When code blocks appear inside function calls or parenthesized expressions, the formatter applies smart indentation to avoid excessive nesting:

```r
# Before formatting
apply(data,1,function(row){
if(condition(row)){
transform(row)
}
})

# After formatting
apply(data, 1, function(row) {
  if (condition(row)) {
    transform(row)
  }
})
```

### Empty Constructs
- **Empty blocks**: `{}` formatting is consistent
- **Empty parameter lists**: `function() {}` maintains compact form
- **Empty argument lists**: `call()` remains unchanged

### Expression Sequences
Semicolon-separated expressions receive appropriate formatting:

```r
# Before formatting
{initialize();process();cleanup()}

# After formatting
{ initialize(); process(); cleanup() }
```

### R6 Class Definitions
Class definitions with empty lines between methods are preserved:

```r
# Formatting preserves intentional spacing in class definitions
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
- **Formula objects**: Proper spacing around `~` operator based on complexity
- **S4 slot access**: `@` operator formatting maintained

## Auto-Bracing

The formatter automatically adds braces to control flow structures when they improve clarity and consistency:

**Function definitions**: Multi-line functions always receive braces, even when the body starts on the same line:

```r
# Before formatting
f <- function(x) 
  x + 1

# After formatting  
f <- function(x) {
  x + 1
}
```

**Conditional statements**: Multi-line conditions or bodies are automatically braced:

```r
# Before formatting
if (condition)
  single_statement

# After formatting
if (condition) {
  single_statement
}
```

**Loops**: All loop bodies are automatically braced for consistency:

```r
# Before formatting
for (i in 1:n)
  process(i)

# After formatting
for (i in 1:n) {
  process(i)
}
```

## Hugging Behavior

"Hugging" refers to how nested function calls are formatted - keeping them compact by allowing the inner calls to start on the same line as the outer call's opening parenthesis. This is part of roughly's non-invasive approach: both hugged and expanded formats are allowed.

**Nested function calls** can be formatted in a hugged style:

```r
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

**Non-invasive multi-line formatting**: When expressions are already multi-line, roughly only adds necessary spacing but preserves the overall structure. However, if all arguments don't fit on their separate lines, they will be properly separated:

```r
# Before formatting - mixed line breaks
call(
  a=x,
  b=y, c=z)

# After formatting - proper spacing and line breaks
call(
  a = x,
  b = y,
  c = z
)
```

**Mixed line formats** are allowed when the last argument starts on the same line:

```r
# This format is preserved - last argument starts on same line
call(a = x, b = y, c = inner(
  expr
))
```

This behavior is particularly useful for testing frameworks and S4 method definitions:

```r
# testthat example
test_that("description", {
  expect_equal(result, expected)
})

# S4 setMethod example  
setMethod("show", "MyClass", function(object) {
  cat("MyClass object\n")
})
```

## Line Endings

The formatter automatically detects and preserves the line ending style (`LF` or `CRLF`) used in the original file.

## Configuration

For details on configuring the formatter, see the [Configuration](/configuration) page.
