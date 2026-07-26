---
title: Getting started
description: What ry is, what it finds on your first run, and where to install it
---

ry is a toolchain for R, written in Rust. It is four tools in one binary:

1. **A language server** — hover, completion, go-to-definition, references, rename, and inlay hints,
   in any editor that speaks LSP.
2. **A formatter** — one style, no options to argue about.
3. **An R console** — a REPL whose completion knows your project.
4. **A type checker** — optional, and the reason the other three know what your code means.

It needs no R installation and no changes to your code, so the same binary runs in your editor and in
CI.

## Your first run

No configuration, no annotations:

```bash
ry check
```

```r
apply_discount <- function(price, rate) {
  price * ratee
}

apply_discount(100, 0.2)
```

```text
warning[unresolved]: I could not resolve `ratee` in this package, its imports, or builtins. Did you mean `rate`?
 --> discount.R:2:11
2 |   price * ratee
              ^^^^^

1 problem in 1 file
```

One transposed letter, found without running anything. R would have found it too — halfway through the
job, on whichever machine ran it first.

## Now turn on the type checker

Fix the typo and make a different mistake — the kind no linter can catch, because it needs to know what
a value *is*:

```toml
# ry.toml
[check]
typing = true
```

```r
apply_discount(100, "0.2")
```

```text
error[type-mismatch]: expected `double`, found `character`
 --> discount.R:5:21
5 | apply_discount(100, "0.2")
                        ^^^^^
```

Nothing was annotated. ry worked out that `rate` is a number because you multiply by it — and
that same knowledge is what makes hover, completion, and the console's tab completion useful. It runs
whether or not you turn the errors on.

## Install

- **VS Code** — the [ry extension](https://marketplace.visualstudio.com/items?itemName=felix-andreas.ry), which bundles the binary.
- **Zed** — from [`editors/zed`](https://github.com/felix-andreas/ry/tree/main/editors/zed); not in Zed's registry yet.
- **Command line** — a prebuilt binary from [Releases](https://github.com/felix-andreas/ry/releases), or `cargo install --git https://github.com/felix-andreas/ry ry`.

Everything else — other architectures, RStudio, building from source — is on the
[installation page](/installation).

## Next

- [Features](/features) — everything you get before configuring anything
- [Tutorial](/type-checking/tutorial) — the type checker on real code
- [Why ry](/why-ry) — why this exists, and how far along it is
