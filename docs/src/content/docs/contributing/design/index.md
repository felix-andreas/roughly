---
title: Design drafts
description: "Working documents for features that are not settled: proposals, open questions, and designs in progress"
---

These are working documents, not contracts. Everything else in the documentation
describes what ry does today and is kept accurate; the pages below describe what
it might do, what is still undecided, and why. They are deliberately absent from
the sidebar — this page is the way in.

Read them for the reasoning, not for current behaviour. Where a draft and the
rest of the documentation disagree, the rest of the documentation is correct. The
authoritative typing contract is the [type system reference](/reference/type-system/),
and what the checker cannot do yet is listed under
[limitations](/type-checking/limitations/).

A draft leaves this folder in one of two directions: its design gets built and
graduates into the reference pages, or it gets declined and the reasoning stays
here as the record of why.

## Drafts

### [Open type-system questions](/contributing/design/open-questions/)

Type-system questions with no settled answer yet, each with the options on the
table and the current stopgap. Tagged unions, S3 dispatch, data frame and matrix
modelling, the variadic `...` body, and the import model. Traits are here too as
a closed entry: declined rather than deferred, with the reasoning recorded.

### [Data masking](/contributing/design/data-masking/)

Checking non-standard evaluation, where a bare name inside `dt[...]` or a dplyr
verb is a column reference no lexical scope can see. Part of this has shipped and
is described in the reference; this page is the sketchpad for the rung that has
not, which needs column vocabularies and depends on the data frame row-type
design.

### [Inline type syntax](/contributing/design/inline-type-syntax/)

A proposal for writing types inline instead of in `#:` comments, as a compiled
dialect with its own file extension. **Its own recommendation is not to build it**
for inline typing alone — the ergonomic case does not survive scrutiny. It is kept
because the machinery it would need is also what checked record constructors and
tagged unions would need, so the costing is reusable if those are ever wanted.

### [REPL design](/contributing/design/repl/)

How `ry repl` loads R at runtime rather than linking against it at build time,
which is what keeps the rest of the workspace free of an R dependency. The console
is shipped, so most of this page is a design record; the open part is wiring the
analysis stack into it.
