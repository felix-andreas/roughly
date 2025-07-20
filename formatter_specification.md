# Roughly Formatter Behavioral Specification

This document provides a detailed specification of how the Roughly formatter transforms R code. It documents the exact formatting behavior for each node type based on the implementation in `format.rs`.

## Overview

The formatter follows a non-invasive approach: it only introduces line breaks when expressions are already multi-line, and preserves single-line expressions where possible. The formatter uses a tree-walking approach with context-aware formatting decisions.

## Core Principles

1. **Non-invasive formatting**: Single-line expressions remain single-line unless already multi-line
2. **Consistent spacing**: Standardizes spacing around operators, after commas, etc.
3. **Indentation**: Uses configurable indentation (default 2 spaces)
4. **Comment preservation**: Maintains comments while reformatting their spacing
5. **Line ending detection**: Automatically detects and preserves LF vs CRLF

## Formatting Rules by Node Type

### Special Nodes

#### IDENTIFIER
- **Behavior**: Preserves raw content unchanged
- **Example**: `variable_name` → `variable_name`

#### COMMENT
- **Behavior**: Reformats comment spacing while preserving content
- **Rules**:
  - `#foo` → `# foo` (adds space after #)
  - `#'foo` → `#' foo` (adds space after #' for roxygen comments)
  - `#*foo` → `#* foo` (adds space after #*)
  - `#:foo` → `#: foo` (adds space after #:)
  - Preserves special comments: `##`, `###`, `#!/usr/bin/env Rscript`
  - Avoids formatting if content contains quotes: `#'foo'` remains unchanged
- **Examples**:
  ```r
  #foo        → # foo
  #'comment   → #' comment
  ##header    → ##header
  ```

#### COMMA
- **Behavior**: Always outputs `,`

### Literals

#### Boolean and Special Values
- `TRUE` → `TRUE`
- `FALSE` → `FALSE`
- `NULL` → `NULL`
- `INF` → `Inf`
- `NAN` → `NaN`

#### Numeric Literals
- **INTEGER, COMPLEX, FLOAT**: Preserve raw content
- **Examples**: `123L`, `3.14`, `2+3i`

#### STRING
- **Behavior**: Smart quote selection based on content
- **Rules**:
  - Prefers single quotes unless content has unescaped double quotes
  - If all double quotes in content are escaped, uses double quotes
  - Otherwise uses single quotes
- **Examples**:
  ```r
  "hello"     → "hello"
  '"quoted"'  → '"quoted"'
  "can't"     → "can't"
  ```

#### NA
- **Behavior**: Preserves raw content (handles `NA`, `NA_real_`, etc.)

### Keywords

#### Control Flow Keywords
- `RETURN` → `return`
- `NEXT` → `next`
- `BREAK` → `break`

#### Special Operators
- `DOTS` → `...`
- `DOT_DOT_I`: Preserves raw content (`..1`, `..2`, etc.)

### Compound Expressions

#### ARGUMENT / PARAMETER
- **Behavior**: Formats function call arguments and parameter definitions
- **Rules**:
  - Adds space around `=` in named arguments: `a=1` → `a = 1`
  - Handles comments with appropriate indentation
  - If comment appears before value, indents the value
- **Structure**: `name = value` or `name = default`

#### ARGUMENTS / PARAMETERS
- **Behavior**: Formats argument/parameter lists with smart multiline handling
- **Rules**:
  - **Hugging**: For ARGUMENTS, if last non-comment child's value starts on same line as opening paren, keeps compact format
  - **Multiline detection**: Checks if opening and closing are on different lines
  - **Empty lists**: `()` remains `()`
  - **Single line**: `function(a = 1, b = 2)`
  - **Multiline**: 
    ```r
    function(
      a = 1,
      b = 2
    )
    ```
  - **Comment handling**: Comments trigger proper indentation

#### BINARY_OPERATOR
- **Behavior**: Formats binary operations with appropriate spacing
- **Rules**:
  - **Spacing**: Adds spaces around most operators except `:` and `^`
  - **Line breaking**: If operator and RHS are on different lines, indents RHS
  - **No spacing operators**: `:` (range), `^` (power)
  - **Spaced operators**: `+`, `-`, `*`, `/`, `<-`, `->`, `%>%`, `|>`, etc.
- **Examples**:
  ```r
  x<-1        → x <- 1
  1+2         → 1 + 2
  1:10        → 1:10
  x^2         → x^2
  ```

#### BRACED_EXPRESSION
- **Behavior**: Formats code blocks with smart hugging
- **Rules**:
  - **Hugging**: If content fits on one line and starts/ends on same line as braces, keeps compact
  - **Empty blocks**: `{}` remains `{}`
  - **Single line**: `{ expression }` 
  - **Multiline**: 
    ```r
    {
      expression1
      expression2
    }
    ```
  - **Semicolon handling**: `{ expr1; expr2 }` for single line, separate lines for multiline

#### CALL / SUBSET / SUBSET2
- **Behavior**: Formats function calls and subsetting operations
- **Rules**:
  - **Indentation**: If function is EXTRACT_OPERATOR with multiline structure, adds extra indentation
  - **Arguments**: Delegates to ARGUMENTS formatting
- **Examples**:
  ```r
  function(arg1, arg2)
  object[index]
  list[[key]]
  ```

#### EXTRACT_OPERATOR / NAMESPACE_OPERATOR
- **Behavior**: Formats member access and namespace operators
- **Rules**:
  - **EXTRACT_OPERATOR**: `$`, `@` operators
  - **NAMESPACE_OPERATOR**: `::`, `:::` operators
  - **Multiline**: If LHS and RHS on different lines, indents RHS
- **Examples**:
  ```r
  object$member
  package::function
  ```

#### FOR_STATEMENT
- **Behavior**: Formats for loops with block enforcement
- **Rules**:
  - **Block enforcement**: Body is always wrapped in braces if not already a braced expression
  - **Condition formatting**: Handles multiline conditions with proper indentation
  - **Keyword spacing**: `for (variable in sequence)`
- **Structure**:
  ```r
  for (variable in sequence) {
    body
  }
  ```

#### FUNCTION_DEFINITION
- **Behavior**: Formats function definitions
- **Rules**:
  - **Multiline detection**: Based on whether function spans multiple lines
  - **Body handling**: If multiline, wraps non-braced bodies in braces
  - **Same line detection**: Checks if name and body are on same line
- **Examples**:
  ```r
  function(x) x + 1
  function(x) {
    x + 1
  }
  ```

#### IF_STATEMENT
- **Behavior**: Formats conditional statements
- **Rules**:
  - **Condition hugging**: If condition fits on one line without comments, hugs parentheses
  - **Block enforcement**: If multiline or has multiline condition, enforces braces for non-braced bodies
  - **Else handling**: Properly formats else and else-if chains
- **Examples**:
  ```r
  if (condition) { body }
  if (condition) {
    body
  } else {
    alternative
  }
  ```

#### PARENTHESIZED_EXPRESSION
- **Behavior**: Formats parenthesized expressions
- **Rules**:
  - **Hugging**: If content fits on one line, hugs parentheses
  - **Multiline**: Indents content if spans multiple lines
- **Examples**:
  ```r
  (expression)
  (
    multiline_expression
  )
  ```

#### PROGRAM
- **Behavior**: Formats top-level program structure
- **Rules**:
  - **Line spacing**: Maintains 1-2 empty lines between top-level expressions
  - **Comment handling**: Handles leading/trailing comments
  - **Final newline**: Always ends with newline

#### REPEAT_STATEMENT
- **Behavior**: Formats repeat loops
- **Rules**:
  - **Block enforcement**: Body is always wrapped in braces if not already braced
- **Structure**:
  ```r
  repeat {
    body
  }
  ```

#### UNARY_OPERATOR
- **Behavior**: Formats unary operations
- **Rules**:
  - **Spacing**: `~` operator gets space before non-identifier operands
  - **No spacing**: `!`, `+`, `-` generally have no space
  - **Special case**: `~ identifier` has no space, `~ expression` has space
- **Examples**:
  ```r
  !condition
  -number
  ~ formula
  ```

#### WHILE_STATEMENT
- **Behavior**: Formats while loops
- **Rules**:
  - **Condition hugging**: Similar to IF_STATEMENT hugging rules
  - **Block enforcement**: Body is always wrapped in braces if not already braced
- **Structure**:
  ```r
  while (condition) {
    body
  }
  ```

## Comment Handling

### General Rules
- Comments are preserved and repositioned appropriately
- Inline comments stay inline when possible
- Comments that cause line breaks trigger proper indentation
- `# fmt: skip` directive disables formatting for specific lines/expressions

### Comment Positioning
- **Before expression**: Triggers newline and indentation
- **After expression on same line**: Preserved with space
- **Between components**: Handled contextually per expression type

## Line Breaking and Indentation

### Multiline Detection
- Expressions are considered multiline if they span multiple lines in the source
- Some constructs (loops, multiline conditionals) force multiline formatting

### Indentation Rules
- Default 2 spaces per level (configurable)
- Nested expressions increase indentation level
- Comments inside expressions get appropriate indentation

### Line Ending Preservation
- Auto-detects LF vs CRLF from source
- Preserves the detected line ending style throughout

## Error Handling

### Syntax Errors
- Formatter stops on syntax errors and reports location
- Missing nodes (incomplete syntax) are reported with location

### Unknown Node Types
- Unknown node types cause formatter to error with node details
- This ensures formatter stays in sync with grammar changes

## Format Skip Directive

### Usage
- `# fmt: skip` comment disables formatting
- Can be placed before a line or at end of line
- Must be on its own line or be the only comment on a line

### Scope
- Affects the immediately following expression
- Or the expression on the same line if placed at end of line