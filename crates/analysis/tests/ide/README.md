# IDE fixtures

This suite covers editor-facing queries over retained `analysis` state.

Use `MultiFile` fixtures with `#!!!!` actions:

- `hover`
  - request body: `path:line:column`
  - output: rendered hover markdown
- `completion`
  - request body: `path:line:column`
  - output: one completion label per line
- `rename`
  - request body: `path:line:column -> new_name`
  - output: affected files only, rendered as `before:` then `after:`
- `goto_definition`
  - request body: `path:line:column`
  - output: one `path:line:column..line:column` location per line, or `no definition`
- `references`
  - request body: `path:line:column`
  - output: every occurrence as `path:line:column..line:column`, declarations marked with
    ` [declaration]`, or `no references`

Group names must be unique across the whole `ide` suite, so prefix them with the feature
(`definition_locals`, `references_globals`, ...) when the same semantic grouping exists for
several actions.

Keep multiple actions in one fixture when they share same workspace snapshot.

## Coverage matrix

Definition, references, and rename all resolve through the same symbol-target machinery, so each
of those suites covers the shared scope matrix plus its feature-specific rendering:

- local bindings: `<-` assignment, `=` assignment, parameters, for-variables, shadowing,
  sequential rebinding, sibling scopes, conditional (maybe-undefined) bindings
- package globals: cross-file use, use inside functions, exported-binding identity,
  same-name redefinition within one file and across files (last export wins; shadowed
  declarations stay local)
- non-targets: undefined names, builtins, extract/`$`/namespace RHS, keywords, literals
- workspace generations: definition after `edit` and `delete`, references after `move`
  (hover covers `move` in its workspace group)

Hover additionally covers every `ExpressionKind` rendering (literals, string-literal names,
calls, subsets, dollar, blocks, unary minus, control flow), typing-comment annotations
(checked, `@if-unknown`, `@trust`, `@new`), `@type`/`@alias` definition hovers including
generics, the maybe-undefined naming warning, package-resolution lines, and the no-hover case
outside expressions.

Completion covers prefix matching from parameters, locals, globals, and keywords across scope
shapes, the `$`/`@`/`::` trigger contexts (including the empty query and the single-colon
non-context), case-insensitive prefixes, keyword/local/global source ordering, global
function-versus-variable kinds, and the no-completions case.

When the symbol-target machinery gains a capability (new binding form, new scope rule), add the
case to definition, references, and rename together so the suites stay aligned.
