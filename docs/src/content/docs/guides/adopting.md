---
title: Adopting an existing codebase
description: How to turn type checking on for a project that has never had it, without drowning in findings
---

Turning `typing = true` on a codebase that has never been type-checked will report findings, possibly
many. That is not a reason to avoid it. It is a reason to do it in four steps rather than one.

## 1. Start with no configuration at all

Code analysis needs no opt-in. Run `roughly check` with no `roughly.toml` and read what comes back —
unresolved names, unused bindings, and duplicate definitions find real mistakes on day one, and none of
them require you to understand the type system.

Fix those first. It is a short list on most projects, and it clears the noise before you add more.

## 2. Turn typing on, and read before you change anything

```toml
# roughly.toml
[check]
typing = true
```

Resist fixing anything on the first pass. Read the whole list instead. On real code it collapses into a
handful of shapes, and recognising the shape is worth more than fixing the first instance:

| Shape | What it usually means |
| --- | --- |
| A value that may be `NULL` used without a guard | Genuine — the guard is missing, or the value can never actually be `NULL` and the checker cannot see why |
| An empty-list accumulator that later gains fields | `x <- list()` then `x$a <- 1`. The checker sees an empty list. Annotate the accumulator |
| A value inferred as something other than a function, being called | Usually a name that shadows a function, or a dynamic construction the checker cannot follow |

None of these mean your code is wrong. They mean the checker could not convince itself, which is a
different claim.

## 3. Move file by file

You do not have to fix the whole project to benefit from any of it. A directive at the top of a file
overrides the project setting in both directions:

```r
# typing: on
```

```r
# typing: off
```

So you can leave `typing = false` project-wide and opt in the modules you are actively working on, or
turn it on project-wide and exempt the files you are not ready for. Either way the setting travels with
the file, in the file, where the next person will see it.

Work outward from the code you trust least or change most.

## 4. Add strict mode last, and only where it earns its keep

```toml
[check]
typing = true
strict = true
```

Strict mode is a much stronger claim than `typing = true`. It reports every place a value became
`Unknown` — every point where the checker gave up rather than concluded something.

That is exactly what you want on a module you intend to rely on, and exactly what you do not want
across a whole legacy codebase on day one. A clean ordinary run means "I found no contradictions". A
clean strict run means "I understood everything". Only the second is a guarantee.

Strict mode is also the honest way to find out how much of a file is really being checked. Because
`Unknown` is compatible with everything, a data-frame-heavy file can pass cleanly while barely being
checked at all — see [limitations](/type-checking/limitations).

## Where the annotations go

Most code needs none. When you do need one, prefer annotating at the **boundaries** — the exported
function, the constructor, the thing that reads from outside — and let inference carry the interior.
See [domain modeling](/type-checking/domain-modeling) for giving your own types names.
