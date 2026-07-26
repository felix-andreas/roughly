---
title: Features
description: What Roughly gives you before you configure anything — and the one flag that changes the rest
---

Everything on this page works out of the box — no configuration file, no annotations, nothing turned
on. Except the last section, which costs one line.

## It finds real mistakes

`roughly check` builds an understanding of your whole project and reports what does not add up. With no
configuration at all:

| Finding | What it catches |
| --- | --- |
| `unresolved` | A name that resolves nowhere — typos, forgotten imports, a function you deleted. Comes with a "did you mean" suggestion drawn from names actually in scope |
| `unused` | A value you assign and never read, including dead stores inside a function |
| `duplicate` | A top-level name defined twice in the same package |
| `syntax-error` | Parse errors, with a caret under the glyph that broke |
| `trailing-comma` | `f(1, 2,)` — R reads this as a *missing third argument*, not as a stray comma, so it fails the moment that argument is used |
| `assignment-operator` | `=` used where `<-` was meant |
| `boolean-shorthand` | `T` and `F`, which are variables and can be reassigned, unlike `TRUE` and `FALSE` |

Two of these are worth dwelling on, because they need a real analysis rather than a text scan.

`unresolved` knows the difference between a name your project defines, a name a package exports, and a
name that exists nowhere — so it can suggest the right correction instead of guessing. It also
understands `NAMESPACE`: an `importFrom()` naming something a package does not export is an **error**,
because R will refuse to load the package.

`unused` distinguishes a binding that is never read from one read on some paths only, so it does not
fire on the normal shapes R code takes.

More checks are available but off by default — naming style, unused parameters, unused imports,
shadowing a builtin. See [diagnostic codes](/reference/diagnostic-codes) for the full set, and
[configuration](/reference/configuration) for turning them on.

## One style, no debates

`roughly fmt` formats R with no options to argue about. It is deliberately non-invasive: it normalises
spacing, indentation, and bracing without rewriting the structure of your code, and it leaves literate
documents alone entirely. Every rule is written down, with a before and after, in
[formatting rules](/reference/formatting-rules).

## Your editor gets smarter

Install the extension and the language server does the rest.

| | |
| --- | --- |
| **Hover** | The inferred type of whatever is under the cursor |
| **Go to definition** | Jump to a binding, across files |
| **Go to type definition** | Jump from a value to the `@type` that declares it |
| **Find references** and **rename** | Across the whole project, not just the open file |
| **Completion** | Locals, project globals, package exports, and record fields after `$` |
| **Signature help** | The signature of the call you are inside, as you type the arguments |
| **Inlay hints** | Inferred types shown inline after assignments |
| **Quick fixes** | Remove an unused assignment, and other one-keystroke corrections |
| **Outline** and **workspace symbols** | Including S4 and R6 members |
| **Folding** and **document highlights** | The ordinary editor comforts |

Diagnostics arrive in two waves so the editor never feels stalled: the cheap parse-derived findings
publish immediately on every keystroke, and the full project analysis follows when it settles.

## One more thing

Every type on this page — the one hover showed you, the one that made completion offer the right
fields, the one in the inlay hint — came from a type checker that has been running the entire time. You
did not annotate anything, and you did not turn it on.

What you have not turned on is whether it **tells you when the answers disagree**:

```toml
# roughly.toml
[check]
typing = true
```

```r
parse_count <- function(raw) {
  count <- 0L
  if (raw == "unknown") {
    count <- "?"
  }
  count + 1L
}

print(parse_count("12"))
```

```text
error[type-mismatch]: expected a numeric value (`integer` or `double`), found `integer | character`
 --> parse.R:6:3
6 |   count + 1L
      ^^^^^

1 problem in 1 file
```

One line of configuration, no annotations, and no change to the code. That is the whole of the type
checker's entry price.

- [Tutorial](/type-checking/tutorial) — put it on real code
- [Concepts](/type-checking/concepts) — how it works out what it knows
- [Adopting an existing codebase](/guides/adopting) — turning it on without drowning
