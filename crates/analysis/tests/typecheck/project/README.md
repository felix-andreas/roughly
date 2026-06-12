# Typecheck Project Suite

This README is the authoritative contract for the `tests/typecheck/project/` suite.

If the coverage split or intended semantics coverage changes, update this file and
`agent/TESTING.md` in the same session as the fixture changes.

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

## Current gaps

- package winner behavior with conflicting types is not yet covered
- `Collate`-driven file order needs harness support for `DESCRIPTION`
- workspace-edit generations (`#.... vN`) are not used yet

## What does not belong here

- single-file semantics already covered by `expressions`, `bindings`, or `interfaces`
- naming-identity assertions, which belong in `tests/naming/global/`
