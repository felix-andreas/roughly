# Implementation Gaps

This document records where the current implementation differs from `ARCHITECTURE.md`.

`ARCHITECTURE.md` is the authoritative target architecture. This file is only a status document for the current implementation shape. Remove entries as the implementation converges on the architecture.

Keep this file focused on concrete current gaps. Do not turn it into a changelog or a second TODO list.

## Current gaps

### No separate naming phase yet

The architecture calls for a naming phase between lowering and typechecking.

The current implementation does not have that phase yet. Name lookup happens directly during typechecking through symbol-keyed environments.

### The current front end does not produce a clean annotated HIR boundary

The architecture calls for lowering to annotated HIR as the front-end output.

The current implementation mixes several responsibilities across `check.rs` and `lower.rs`:

- annotation syntax validation runs before lowering
- lowering scans source text for annotations again
- lowering reparses annotations and attaches them after expression lowering

The result is close to annotated HIR, but the phase boundary is not clean yet.

### Definition blocks are not represented as first-class declarations yet

The architecture calls for type declarations to appear as first-class front-end items.

The current lowering code models expressions and attached annotations, but it does not yet expose a declaration-oriented representation for `@type` and `@alias` definition blocks.

### Typechecking and inference engine code are still combined

The architecture distinguishes the top-level typechecking phase from the HM-style inference engine it uses internally.

The current implementation keeps those concerns in one module. Expression checking, compatibility, builtin typing, environment handling, unification, instantiation, and generalization are still implemented together.

### File results are diagnostics-only

The architecture expects file checking to retain semantic results that can feed diagnostics, tooling, and later interface extraction.

The current public check result contains diagnostics only. Successful semantic results are not yet exposed as a stable checked-file artifact.

### Shared project analysis state is still minimal

The architecture leaves room for shared analysis state across files and later incremental project checking.

The current analysis state is still file-local in practice. It does not yet model project-level dependency information, checked interfaces, or reusable incremental results.

### The implementation is still centered on direct file checking

The architecture distinguishes the file-local pipeline from later project-level checking and interface extraction.

The current implementation is still primarily a direct single-file checker. The project-level boundary is planned, but not yet realized in code.
