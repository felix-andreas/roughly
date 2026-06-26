# Static Typing for R

This crate explores a static type system for a subset of R.

The goal is to make a useful subset of R statically checkable while keeping the resulting types, diagnostics, and language-tooling output readable to R programmers.

It is intended to stay practical on large code bases, not only on small examples.

This is still a work in progress. The supported language subset, the exact type behavior, and some user-facing details are still evolving.

## Goals

The project is guided by a few broad design goals:

- use Hindley-Milner style inference as the foundation
- aim for a sound type checker within the supported subset
- treat the supported subset as value-like to avoid early variance problems that arise with mutable state
- use JSDoc-style comment annotations, inspired by [TypeScript's supported JSDoc types](https://www.typescriptlang.org/docs/handbook/jsdoc-supported-types.html), so typing does not require changing R syntax or adding a compilation step to run programs
- aim for Rust- and Elm-like diagnostics: clear, precise, and actionable
- preserve semantic information needed for tooling such as hover and inlay hints
- stay viable on very large code bases

It also has clear non-goals for v1:

- full coverage of base R syntax and semantics
- S3 dispatch modeling
- S4 dispatch modeling
- NSE and metaprogramming completeness
- environment and reference semantics

The `analysis` design docs (architecture, file structure, and the testing contract) live in the Roughly [docs site](https://roughly.felixandreas.me) under Contributing; the agent knowledge base lives in `.agents/memory/`.
For the current supported semantics, see the [Typing Reference](https://roughly.felixandreas.me/typing-reference). Today the checker focuses on a small set of ideas:

- atomic R types such as `logical`, `integer`, `double`, and `character`
- scalar-like, array-like, and map-like vector shapes
- tuple-like, record-like, array-like, and map-like `list(...)` values
- function types written in `#:` comments
- explicit `Any`, `Unknown`, and `NULL`
- nullable unions written as `T | NULL`
