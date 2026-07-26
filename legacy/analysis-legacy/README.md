# analysis

The analysis engine behind [Roughly](https://roughly.felixandreas.me): parsing, lowering to HIR,
name resolution, a Hindley–Milner static type checker for R, and the IDE-query layer (hover,
completion, goto-definition, references, rename, inlay hints, signature help) that the language
server and the CLI serve.

Because R has no type-annotation syntax, annotations are written in `#:` comments using a
JSDoc-like notation, so annotated code stays fully compatible with ordinary R tooling. Standard
library signatures come from a declaration-only `.Rtypes` stub corpus that projects can extend
and override.

## Design goals

- Hindley–Milner inference as the foundation; sound within the supported subset
  (unsupported constructs are refused loudly, never silently mistyped)
- Rust- and Elm-quality diagnostics: clear, precise, actionable wording with precise ranges
- preserve the semantic information editor features need (hover, inlay hints, completion detail)
- stay fast on very large code bases — the sibling `engine` crate memoizes these passes
  incrementally; this crate also provides the from-scratch checker used by the CLI batch path
  and as the differential-testing oracle

## Where things are documented

The authoritative, always-current documentation lives on the docs site:

- [Typing Reference](https://roughly.felixandreas.me/reference/type-system) — the semantics contract:
  supported types, unions, vector and list shapes, guard narrowing, strict mode, data-masked
  evaluation (NSE), per-file typing directives
- [Typing guide](https://roughly.felixandreas.me/type-checking/tutorial) — the tutorial
- Contributing pages — architecture, this crate's file structure, and the fixture-testing
  contract

## Out of scope (for now)

- S3/S4 dispatch modeling (S4 slots are lowered; dispatch is not)
- NSE and metaprogramming completeness beyond the recognized data-masking forms
- environment and reference semantics
