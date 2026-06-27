# Typecheck Project Suite

This README is the authoritative contract for the `tests/typecheck/project/` suite.

If the coverage split or intended semantics coverage changes, update this file and
the testing docs page in the same session as the fixture changes.

## Purpose

`project` answers what multi-file, package-visible typed behavior looks like:

- cross-file value use through package-global naming
- cross-file `@type` and `@alias` use through the project-global type namespace
- later-file winner behavior for repeated top-level names
- script (non-package) documents consuming package globals without contributing to them

## Rendered output contract

Cases are `MultiFile` fixtures running the full analysis pipeline (`lint`, `lower`, `naming`,
`typecheck`) with typing enabled. Each file's expectation is its rendered diagnostics, with
`No diagnostics.` for clean files.

The check pipeline currently retains no typed artifact for successful files, so this suite cannot
yet render cross-file typed snapshots or exported package surfaces. When checked-file results
exist, revisit this contract.

## Current files

- `values.R.test` — cross-file value use, argument mismatches across files, later-file visibility
- `types.R.test` — cross-file alias and nominal use, cross-file `@new`, forward type references
- `scripts.R.test` — scripts read package globals; script bindings and type declarations stay
  script-local; package and script files may reuse a type name
- `audit_cross_file.R.test` — audited cross-file diagnostics
- `incremental.R.test` — workspace-edit generations: interface vs body edits, deletes, and forward
  type definitions across generations
- `recompute_scope.R.test` — the M3 recompute-scope exit proof: an interface edit recomputes the
  edited file plus its referrers (`k + 1`), a body-only edit recomputes just the edited file

## The `recompute` action

A generation may assert which documents the incremental typecheck recomputed, and why, with a
project-suite IDE action:

```text
#!!!! recompute recompute.scope
#++++
R/a.R: body-edit
R/b.R: interface-change(double_count)
```

- `#!!!! recompute <path>` takes no request body; `<path>` is only the output key for the rendered
  scope within the snapshot
- the output lists every recomputed document, sorted by path, as `path: reason`
- the reason is `body-edit` when the document's own version changed, or
  `interface-change(name, ...)` naming the changed package-globals it references that forced the
  recheck (a document attributed to its own edit is `body-edit` even if it also references a changed
  global)
- like other IDE actions the scope is snapshot-local and does not carry forward

## Current gaps

- package winner behavior with conflicting types is not yet covered
- `Collate`-driven file order needs harness support for `DESCRIPTION`

## What does not belong here

- single-file semantics already covered by `expressions`, `bindings`, or `interfaces`
- naming-identity assertions, which belong in `tests/naming/global/`
