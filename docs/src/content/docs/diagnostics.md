---
title: Diagnostic codes
description: Every code Roughly emits, what turns it on, and how to silence it
---

Every diagnostic Roughly reports carries a **stable code**, shown in brackets in the message:

```text
warning[unused]: `helper` is assigned but never used.
```

The code is a contract. It is what you write in a suppression comment, what you set in
`roughly.toml`, and what appears as `code` in `--output json` — so it is safe to build tooling on,
and it does not change when the wording of a message improves.

## Analysis codes

These come from the checker itself. They are not individually configurable; the `[check]` switches
turn whole classes on or off.

| Code | Reports | On by default |
| --- | --- | --- |
| `syntax-error` | Code that does not parse. A broken region reports this and nothing else — the checker draws no conclusions from source that failed to parse | Always |
| `annotation` | A malformed `#:` annotation: a type that does not parse, an unknown type name, a `@type`/`@alias` block mixed with checked annotations | Always |
| `unresolved` | A name that resolves nowhere — not in this package, its imports, or the standard library. Includes a "did you mean" suggestion for a near miss. **A `library(pkg)` for a package with no shipped stub tolerates otherwise-unresolved bare names**, since that package's export set is unknowable — except a near miss of a name your own project binds, which is reported anyway | Always |
| `unused` | A binding nothing ever reads. Top-level bindings are reported in scripts but not in package files, where any file may use them | Yes — `[check] unused = false` opts out |
| `duplicate` | Two top-level definitions of one name in a **package**, reported at both sites — a script is sequential, so rebinding there is ordinary. Also two `@type`/`@alias` declarations of one type name | Always |
| `stub` | A problem in a `.Rtypes` stub the project ships — reported against the stub, since a stub that fails to load silently withdraws the types it promised | Always |
| `type-mismatch` | A type error: incompatible argument, missing or surplus argument, calling a non-function, an operator applied to operands it is not defined for | No — `[check] typing = true` |
| `strict` | A place where the checker genuinely could not determine a type, so the value became `Unknown` and checks there were skipped | No — `[check] strict = true` |

`strict` deserves a note: it does not find *errors*, it finds *silence*. Turning it on is how you
learn how much of a file is actually being checked. See [strict mode](/typing/reference#strict-mode)
and [limitations](/limitations).

## Lint codes

These are individually configurable in the `[lint]` table, each set to `"off"`, `"warn"` or
`"error"`:

```toml
[lint]
assignment-operator = "off"
unused-parameter = "warn"
```

| Code | Reports | Default |
| --- | --- | --- |
| `assignment-operator` | `=` used for assignment where `<-` is meant | warning |
| `boolean-shorthand` | `T` and `F` instead of `TRUE` and `FALSE` — both are ordinary variables in R and can be reassigned | warning |
| `trailing-comma` | A comma after the last argument, which in R supplies a missing argument rather than being ignored | **error** |
| `naming-style` | A variable or parameter that does not match the configured style. Configured by value, not level: `[lint] naming-style = "snake_case"` or `"camelCase"`. `SCREAMING_SNAKE_CASE` conforms under either | off |
| `unused-parameter` | A parameter no read ever uses. Off because R signatures legitimately carry ignored formals; `.`/`_`-prefixed names and S3 method formals are never reported | off |
| `unused-import` | An `importFrom(pkg, name)` in the `NAMESPACE` whose name appears in no source. Usage detection is a conservative token scan, so it never fires on a real use | off |
| `shadows-builtin` | A top-level binding whose name `base` exports | off |
| `shadows-namespace` | The same, for names from the other standard-library namespaces (`stats::filter`, `utils::head`) | off |

See the [linter page](/linter) for what each one looks like in practice, and
[configuration](/configuration) for the full `roughly.toml` surface.

## Silencing a finding

A `# roughly: allow(...)` comment suppresses the named codes on the line it precedes or the line it
ends:

```r
# roughly: allow(unused)
scratch <- compute()

total = 1L  # roughly: allow(assignment-operator)
```

Several codes separated by commas work: `# roughly: allow(unused, naming-style)`, and
`# roughly: allow(all)` suppresses every finding on that line.

### Testing that something is rejected

A test that asserts bad input is refused contains a call that really is type-incorrect, so the
finding is true — it is just not what you want to hear:

```r
test_that("bump rejects a string", {
  # roughly: allow(type-mismatch)
  expect_error(bump("x"))
})
```

The suppression is deliberate rather than automatic. Silencing type findings inside every
`expect_error(...)` would also silence a genuine mistake in the test — a misspelled function name,
or the wrong argument passed by accident — and those are worth catching in test code as much as
anywhere else.

To silence a lint everywhere instead, set it to `"off"` in `[lint]`. To silence a whole class, use
the `[check]` switches — or, for one file, the `# typing: off` directive at the top of it.

## In CI

`--output json` emits one object per diagnostic with the same `code` field, so a CI job can route
or count findings by code without parsing messages. Both `check` and `fmt --check` exit 1 on any
finding, warnings included; `--min-severity error` narrows that to errors. See
[continuous integration](/installation#continuous-integration).
