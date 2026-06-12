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

Definition, references, and rename all resolve through the same symbol-target machinery, so the
suites focus on per-feature rendering plus the scope matrix: local bindings (assignment,
parameter, for-variable, shadowing, sequential rebinding), package globals (cross-file use,
exported-binding identity), and non-targets (undefined names, builtins, extract/namespace RHS).
