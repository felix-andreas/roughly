---
title: Concepts
description: How Roughly's type system works — the vocabulary you need to read what it tells you
---

This page explains the type system. It is meant to be enough on its own: you should be able to read
everything the checker says without opening the [specification](/reference/type-system).

## Use decides type

Roughly does not require you to declare types. It works out what a value must be from what you do with
it:

```r
scale <- function(x, factor) {
  x * factor
}
```

`*` is arithmetic, so `x` and `factor` are numbers. That is inferred, not declared, and it is enough to
reject `scale("a", 2)`.

This is the single most important thing to understand about the system, because it explains why most R
needs no annotations at all. You annotate where you want to say something the code does not already
say — a boundary, a domain type, a promise you want held to.

## R has no scalars, so the type system draws the line itself

In R, `1L` is an integer vector of length one. There is no separate scalar type, which means the type
system has to choose a convention. Roughly's is:

| Notation | Means |
| --- | --- |
| `integer` | An integer vector of **length one** |
| `integer[]` | An integer vector of **unknown length** |
| `integer[named]` | An integer vector of unknown length, **keyed by names** |

Literals are scalars. `1L` is `integer`, not `integer[]`.

The relationship between them is **one-way**: a scalar is accepted where a vector is expected, because
a length-one vector is a vector. The reverse is not — a vector of unknown length is not accepted where
a single value is required, because it might have length zero, or seven.

## Atomic types and the numeric ladder

The atomic types are `logical`, `integer`, `double`, `complex`, `character`, and `raw`.

Four of them form a ladder, and values widen up it implicitly:

```
logical  <  integer  <  double  <  complex
```

So an `integer` is accepted where a `double` is wanted. `character` and `raw` are deliberately **not**
on the ladder — R will happily coerce a number to a string, but doing it silently is almost always a
mistake rather than an intention, so Roughly makes you say it.

Widening happens only when checking whether a value *fits* somewhere. It never happens when the checker
is working out what two things have in common — that is a different question, and answering it by
widening would quietly lose information.

## Containers: fixed shape or unknown shape

This is the distinction that trips people up, so it is worth being explicit. Lists come in two flavours,
and the difference is whether the checker knows the shape.

**Homogeneous** — every element the same type, length unknown:

| Notation | Means |
| --- | --- |
| `integer[]` | Integer vector |
| `list[integer]` | List of integers |
| `list[named: integer]` | List of integers keyed by names |

**Heterogeneous** — a fixed shape the checker tracks element by element:

| Notation | Means | Called |
| --- | --- | --- |
| `list{integer, character}` | Exactly two elements, of those types | A tuple |
| `list{name: character, age: integer}` | Exactly those fields, of those types | A record |

A list literal infers the fixed shape, which is what makes `$` work:

```r
person <- list(fullname = "Ada", age = 36L)
person$fullnme
```

`person` is `list{fullname: character, age: integer}`, so the checker knows the field does not exist —
instead of the silent `NULL` you would get at runtime:

```text
error[type-mismatch]: field `fullnme` does not exist in `list{fullname: character, age: integer}`. Did you mean `fullname`?
```

## Functions

A function type is written the way you would say it out loud:

```
fn(x: integer, y: character) -> logical
```

Optional parameters — those with defaults — are marked, and parameter **names** are part of the type,
because R calls functions by name as often as by position.

## Unions and `NULL`

A value that could be one of several things has a union type, written with `|`:

```
integer | character
```

You have already seen one: when a variable is assigned different types on different branches, its type
after the `if` is the union of both. That is what the type checker reports in the
[Features](/features) example.

`NULL` is its own type, and this is where unions earn their keep. A function that may return nothing
has type `character | NULL`, and using that result as a `character` without checking is an error — the
missing `if (is.null(x))` that would have failed at runtime.

## `Any` and `Unknown`

Both accept anything and are accepted anywhere. They exist as two names because the *reason* differs,
and the reason is what you need to know:

| | Means |
| --- | --- |
| `Unknown` | **The checker could not work it out.** A gap in its knowledge |
| `Any` | **You said not to care.** A deliberate opt-out you wrote |

This matters more than it looks. Because `Unknown` is compatible with everything, one construct the
checker cannot model does not cascade into a screen of errors — it just goes quiet. That is what keeps
the tool usable on real R. It also means a clean run does not by itself tell you how much was actually
checked.

[Strict mode](#typing-modes) is the answer to that: it reports every place a value became `Unknown`, so
you can see the gaps instead of mistaking them for approval.

## Naming your own types

Two ways, and choosing between them is a real decision.

**`@alias` is transparent.** The name is shorthand; it expands to its body everywhere, and the alias and
its body are freely interchangeable:

```r
#: @alias UserId {integer}
```

A `UserId` *is* an `integer`. Anywhere one works, so does the other.

**`@type` is nominal.** The name is a distinct type, even if its representation is identical to
something else:

```r
#: @type Celsius {double}
#: @type Fahrenheit {double}
```

A `Celsius` is **not** a `Fahrenheit`, and neither is a bare `double`. That is the point — it is how you
stop the two being mixed up, which no amount of structural checking can do for you.

Because a nominal type is distinct, a structural value never becomes one by accident. The only way in is
to say so:

```r
#: @type Person {list{name: character, age: integer}}

#: @new Person
ada <- list(name = "Ada", age = 36L)
```

`@new` checks the value against the representation and then hands you the nominal type. It is the
single door in, which is exactly what makes the type mean something on the way out. See
[domain modeling](/type-checking/domain-modeling) for using this in anger.

Note the blank line above. A `#:` block commits to one thing: it declares types (`@type`, `@alias`), or
it annotates the item on the next line — never both. Run them together and you get
`error[annotation]: @type and @alias declarations need their own #: block`.

## Narrowing

Inside an `if`, the checker learns from the condition:

```r
greet <- function(name) {
  if (is.null(name)) {
    return("hello, stranger")
  }
  paste0("hello, ", name)
}
```

After the guard, `name` is no longer `NULL` and `paste0` is happy.

Narrowing is deliberately narrow. It happens on the condition of an `if`, and only for a guard applied
directly to a plain variable. `if (is.null(x))` narrows `x`; `if (is.null(obj$field))` does not, and
neither does a guard behind an `&&`. If a narrowing you expected did not happen, this is almost always
why — lift the value into a local variable first.

## Generics

You rarely write these, and you get them anyway. An unannotated function whose parameters nothing
constrains is generic automatically:

```r
identity2 <- function(x) x
```

is `<T> fn(x: T) -> T` — it returns exactly what you gave it, whatever that was. Add one arithmetic
operation and the checker narrows the promise on its own:

```r
increment <- function(x) x + 1L
```

is `<T: numeric> fn(x: T) -> T`, which says "some number in, the same kind of number out". You do not
have to ask for that, and there is nothing to maintain.

When you do want to write one, the binder goes at the front:

```r
#: <T> fn(items: list[T], index: integer) -> T
```

## Typing modes

Three, and they are per file as well as per project:

| Mode | Reports |
| --- | --- |
| `off` | No type findings. Everything else still runs |
| `on` | Contradictions — places where the types genuinely disagree |
| `strict` | Contradictions, plus every place a value became `Unknown` |

Set the project default in [`roughly.toml`](/reference/configuration), and override it in any single
file with a comment at the top:

```r
# typing: strict
```

The file always wins, which is what lets you adopt this one module at a time.

## Next

- [Tutorial](/type-checking/tutorial) — the same ideas, applied to real code
- [Domain modeling](/type-checking/domain-modeling) — nominal types instead of S4, R6, or S7
- [Limitations](/type-checking/limitations) — where the checker cannot help yet
- [Type system reference](/reference/type-system) — the exact contract
