---
title: Concepts
description: How ry's type system works — the vocabulary you need to read what it tells you
---

## Inference

**Inference** is the checker working out a type instead of being told one. What a value must be follows from what you do with it.

```r
scale <- function(x, factor) {
  x * factor
}
```

`*` is arithmetic, so `x` and `factor` are numbers. That is inferred, not declared, and it is enough to
reject `scale("a", 2)`.

You annotate where you want to say something the code does not already
say — a boundary, a domain type, a promise you want held to.

## Scalars and vectors

In R, `1L` is an integer vector of length one. There is no separate scalar type, which means the type
system has to choose a convention. ry's is:

| Notation | Means |
| --- | --- |
| `integer` | An integer vector of **length one** |
| `integer[]` | An integer vector of **unknown length** |
| `integer[named]` | An integer vector of unknown length, **keyed by names** |

Literals are scalars. `1L` is `integer`, not `integer[]`.

The relationship between them is **one-way**: a scalar is accepted where a vector is expected, because
a length-one vector is a vector. The reverse is not — a vector of unknown length is not accepted where
a single value is required, because it might have length zero, or seven.

## Atomic types

The atomic types are `logical`, `integer`, `double`, `complex`, `character`, and `raw`.

Four of them form a ladder, and values widen up it implicitly:

```
logical  <  integer  <  double  <  complex
```

So an `integer` is accepted where a `double` is wanted. `character` and `raw` are deliberately **not**
on the ladder — R will happily coerce a number to a string, but doing it silently is almost always a
mistake rather than an intention, so ry makes you say it.

Widening happens only when checking whether a value *fits* somewhere. It never happens when the checker
is working out what two things have in common — that is a different question, and answering it by
widening would quietly lose information.

## Lists

R gives you one `list()` and expects you to use it for everything. In practice you use it for at least
four different jobs, and the mistakes you can make with each are different:

| What you are building | Type | Example |
| --- | --- | --- |
| A record with named fields | `list{name: character, age: integer}` | `list(name = "Ada", age = 36L)` |
| A fixed-size tuple | `list{integer, character}` | `list(1L, "ok")` |
| A growable sequence, all one type | `list[integer]` | results you `append()` to in a loop |
| A lookup table keyed by name | `list[named: integer]` | counts by category |

The split that matters is **fixed shape** versus **unknown shape**. `list{...}` means the checker knows
exactly which elements exist, so it can check `$` and `[[`. `list[...]` means it knows the element type
but not how many there are, so it cannot.

Atomic vectors have the same two shapes: `integer` is one value, `integer[]` is many.

**None of this changes anything at runtime.** All four are an ordinary R `list`; `is.list()` is `TRUE`
for every one, `str()` prints what it always printed, and there is nothing to migrate. The distinction
exists only so the checker can tell you which of the four you actually built.

That is what makes `$` safe:

```r
person <- list(fullname = "Ada", age = 36L)
person$fullnme
```

The literal has a fixed shape, so the checker knows the field is not there — instead of the silent
`NULL` you get at runtime:

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

`NULL` is its own type, and this is where unions matter most. A function that may return nothing
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

### `@type`

Start with the case you actually have: a thing in your domain with named fields. In R you would reach
for S4, R6 or S7. ry gives you a third option that the checker can see into:

```r
#: @type Person {list{name: character, age: integer}}

#: fn(name: character, age: integer) -> Person
new_person <- function(name, age) {
  #: @new Person
  list(name = name, age = age)
}

#: fn(p: Person) -> character
greet <- function(p) paste0("hi ", p$name)
```

A `Person` is an ordinary named list at runtime — no class system, no dispatch, no dependency. But it
is a **distinct type** to the checker, so `greet` accepts a `Person` and nothing else:

```r
greet(list(name = "Bob", age = 40L))
```

```text
error[type-mismatch]: expected `Person`, found `list{name: character, age: integer}`
```

Read that error carefully, because it is the whole idea. The list has *exactly* the right fields with
exactly the right types, and it is still rejected. Matching the shape is not enough — you have to have
gone through the door:

```r
#: @new Person
ada <- list(name = "Ada", age = 36L)
```

`@new` is that door. It checks the value against the declared representation and hands back the nominal
type. Put it inside a constructor, as `new_person` does, and every `Person` in your program provably
came from there.

This is **nominal** safety, as opposed to the **structural** kind. Structural typing asks "does it have
the right shape?"; nominal typing asks "is it the thing?". S4 and R6 answer that question at run time and tell the
checker nothing. `@type` answers it at analysis time and adds nothing at run time.

### Two names for the same shape

Nominal types matter most when two values have the same shape but must not be swapped:

```r
#: @type Celsius {double}

#: @type Fahrenheit {double}

#: fn(temp: Celsius) -> Fahrenheit
to_fahrenheit <- function(temp) {
  #: @new Fahrenheit
  temp * 9 / 5 + 32
}

#: @new Celsius
freezing <- 0

warm <- to_fahrenheit(freezing)
to_fahrenheit(warm)
```

```text
error[type-mismatch]: expected `Celsius`, found `Fahrenheit`
```

The last line converts an already-converted temperature — the bug every unit mix-up is. Both types are
`double` underneath, and arithmetic on them still works, because the representation is visible
*outward*. What does not happen is a bare `double`, or the other unit, arriving where one is expected.

### `@alias`

Sometimes you want the opposite: a shorthand that stays interchangeable with what it abbreviates.

```r
#: @alias Row {list{id: integer, label: character}}
```

`Row` expands to its body everywhere, so a `Row` and that list are the same type. Use it to stop
repeating a long type; use `@type` when being confused with the underlying value is the thing you are
trying to prevent.

:::note
A `#:` block does one thing: it declares types (`@type`, `@alias`) **or** it annotates the item on the
next line. Run them together and you get
`error[annotation]: @type and @alias declarations need their own #: block`. Separate them with a blank
line, as above.
:::

See [domain modeling](/type-checking/domain-modeling) for constructors, validation, and when R6 is
still the right answer.

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

Set the project default in [`ry.toml`](/reference/configuration), and override it in any single
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
