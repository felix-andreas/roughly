# Test-user reports — resolved findings

The archive of simulated-user findings that are **closed**. Open ones stay in `backlog.md`; a
finding moves here when it is fixed, so the backlog shows only work that is left while the reports
themselves survive — each entry keeps what was reported, what the measurement actually showed (the
premise was wrong more than once), and what shipped.


## test-user round 3: typing enthusiasts (the type system, not the libraries)

### A. The checker gives a WRONG answer, not a skipped one — two confirmed cases (both fixed)

`limitations.md` sells the whole trust story on one sentence: *"a gap means checks are **skipped**, not
that wrong answers are produced."* Both of these broke it, and both were silent under `strict = true`.
Both are now fixed. Shared lesson worth keeping: each one produced a type belonging to *neither* of the
program's possible states — a stale field type, and a merged signature — and in both cases that single
wrong type generated a false positive and a false negative simultaneously, which is why either could be
mistaken for a mere precision gap when read from one direction only.


1. **FIXED — a field write through a nominal value was discarded, and the stale belief was used both
   ways.** `replacement_written_type` handled `Record` and empty `Tuple` and let everything else fall
   through to `_ => prior`, so a nominal kept its declared field types after a contradicting write. The
   write is now **checked against the representation rather than applied to it**: a nominal's
   representation is fixed, so a value the declared field type refuses is reported at the value, a
   field the representation does not carry is reported by name, and the binding keeps its nominal type
   either way — which is what makes the `@type` invariant hold past construction. Both spellings (`$`
   and `[["literal"]]`) go through the one path; an opaque or non-record representation stays quiet
   (R only warns and coerces for `x$f <- v` on an atomic, so there is nothing to refuse). Structural
   records still retype, which is the contrast the report drew. Reference updated, seven fixtures.


2. **FIXED — a branch join involving a function slot invented a type belonging to neither path.**
   `join_writes_reporting` let a branch's scheme replace whatever preceded it, and a monotype replace a
   scheme, so a conditionally reassigned function kept **one** arm. `pick` above exported
   `fn(shout: logical) -> fn(x: T) -> character` — the parameter from the identity branch, the return
   from the `paste0` branch, a signature neither function has. Both directions followed: a
   character-expecting call on `pick(FALSE)(1L)` was **accepted** though R gives `integer`, and numeric
   use was rejected with ``found `character` ``, a claim about the value that was simply untrue.

   The join now unions the two entries' monotypes, and unions rather than unifying: `join_types` tries
   unification first, and two *instantiated* schemes always unify through their fresh variables
   (`fn(x: T) -> T` with `fn(x: U) -> character` by binding `T := character`), which is exactly how the
   fabricated signature arose. `pick` is now
   `<T, U> fn(shout: logical) -> fn(x: T) -> T | fn(x: U) -> character`, calling it returns
   `integer | character`, and a single reaching write still keeps let-polymorphism. The reference
   already specified this join correctly — the implementation had simply never matched it.

   **The original report's framing was wrong and is corrected here:** this was not "correct code
   rejected". R fails on the other path (`pick(TRUE)(1L) + 1L` is `non-numeric argument to binary
   operator`), so refusing the repro is right; what was broken was the fabricated type and the
   inaccurate message. The repro still reports, now as ``found `integer | character` ``.


### B. False positives on the first thing a newcomer writes


3. **FIXED — guard narrowing did not fire at file top level.** `recognize_guard` required the tested
   read to be in `naming.resolutions`, and a top-level variable read by a *later* statement is not:
   scopes are per-item, so it is a non-local. The guard bailed at the slot lookup, which is why the
   byte-identical guard inside a function body was clean — there, everything is one item.

   A non-local read now gets a checker-minted slot (ids allocated above every id naming issued, so
   they cannot collide), seeded with the type the read observed, and the ordinary environment carries
   the refinement from there — no second place to keep flow state. Null, negated and family guards all
   narrow at top level now.

   **One limit is inherent and is now documented rather than hidden:** a refinement does not outlive
   the statement it was made in, because checking is per top-level statement. So
   `if (is.null(x)) stop(...)` followed by a *separate* top-level statement does not narrow, while the
   same idiom inside a function body or one braced block does. Fixtured both ways. Making it cross
   statements would mean threading flow state between items, which `item_check`'s per-item memoization
   is built to avoid — not worth it for this, but that is the reason, not an oversight.


4. **FIXED, but not as reported — the report's premise was half wrong.** `duplicate_type_diagnostics`
   returned early for any non-package file. The report read that as "the check does not run for script
   projects", implying script declarations should conflict across files like a package's. They should
   not: measured, a script's type declarations reach **only their own file** — an `@alias Shared` in
   `one.R` is not visible from `two.R`, which reports ``I do not know the type `Shared` ``. So two
   scripts may each declare `Thing`, and reporting that as a conflict would have been a new false
   positive.

   The real gap was narrower: a name declared twice **inside one script file**, where the namespace
   genuinely is shared. That went unreported, the later declaration silently won, and the mismatch it
   produced was unfalsifiable — a `Thing` declared `double` above and `character` below yields
   ``expected `character`, found `double` `` with nothing in view to explain it. Both declarations are
   now reported, each pointing at the other, with wording that names the namespace it is judged against.

   Packages already handled the within-file case (the project map counts one file twice), and that path
   is untouched. Three script fixtures plus a CLI test for the cross-file no-conflict half, which the
   fixture suites cannot express because a case is one file. The reference said duplicates are errors
   "regardless of file" while also saying non-package files do not join the project namespace; it now
   states which namespace a duplicate is judged against.


5. **FIXED — `= NULL` is no longer exempt from the default-value check.** This was a documented
   decision, not an oversight ("a `NULL` default is R's 'no value' sentinel … always allowed"), so it
   was weighed rather than flipped, and the measurement settled it: the exemption hides a real crash.
   Declared `[title]: character` with `title = NULL`, the body's `if (title == "draft")` passed the
   checker and failed at run time with R's `argument is of length zero`. Declared
   `character | NULL`, the same body is caught statically, and adding `if (is.null(title))` clears it —
   so the remedy was already fully supported and only the exemption stood in the way.

   The default is now checked like any other, with its own diagnostic rather than a bare mismatch,
   because this is the usual R spelling and the fix is specific: ``` `title` defaults to `NULL`, which
   its declared type `character` does not admit — a caller who omits it leaves `NULL` in the body.
   Declare it `character | NULL` and narrow with `is.null()` ```. `Any` still admits it, an
   unannotated parameter is unaffected, and non-`NULL` defaults are unchanged.

   Three examples **in the reference itself** relied on the exemption and now read `character | NULL`.
   One of them guarded with `if (!is.null(label))` while declaring `label` as plain `character` — the
   spec demonstrating the lie in its own sample is the clearest evidence the rule was wrong.


### C. Annotations that validate themselves and then enforce nothing

6. **FIXED — an annotation in call-argument position is reported instead of silently dropped.** As
   filed, the danger was that a typo *inside* such an annotation IS reported, so the user got positive
   feedback that a block doing nothing was live. Verified per position: a braceless function body, a
   braceless `if` branch and a parenthesised expression all attach; only `ARGUMENT_LIST` drops, so the
   check keys on that alone. It reports rather than attaches — giving a lambda parameter an annotatable
   position is a separate, still-open expressiveness question.

7. **FIXED — an annotation declared the parameter LIST, not just the parameter types.** As filed: one
   bad `@param missing_one` produced 5 errors, four of them telling correct `f(1L)` calls they were
   missing an argument. Reproducing it showed the cascade was one symptom of a general defect — the
   exported signature was the annotation as written, so *every* way the two sides can disagree about
   arity was resolved in the annotation's favour and charged to the call sites:

   | disagreement | before | R |
   | --- | --- | --- |
   | `@param` names a non-existent formal | 1 real error + 1 per call site | — |
   | **only some formals annotated** | 1 error **per call site**, nothing at the definition | calls are fine |
   | more declared types than formals | nothing at the definition, 1 error per call site | calls are fine |
   | `[x]` declared optional, formal has no default | `f()` **accepted** | `argument "x" is missing` |
   | `...` declared, formals fixed | `f(1, 2, 3)` **accepted** | `unused arguments` |

   The partial-annotation row is the one that mattered most: annotating a single `@param` of several
   turned every correct call into a finding, and that is the ordinary way to start annotating.

   The fix is one rule, now in the reference: **an annotation declares the types of a definition's
   parameters, never the parameter list.** R matches arguments against the `function(...)` header, so
   `check_declared_function` builds the exported signature from the formals — their names, order,
   optionality, and `...` position — and fills the types from the declaration, name-aware. Every
   disagreement is reported once at the definition and never again at a call. Two of the five rows
   were false *negatives*, so this also closes call shapes that R rejects and the checker accepted;
   both were verified against R 4.3.3 rather than assumed.

   Two things came with it, both required for the signature to be honest:

   - **An undeclared formal keeps its inferred type.** It is a fresh variable like any unannotated
     parameter, so the export edge (`close_scheme`) either generalizes it or erases it to `Unknown`.
     Partial annotation now *adds* checking instead of removing it: `#: @param x {integer}` on
     `function(x, y) x + y` infers `y: integer` and catches `f(1L, "no")`.
   - **An elided return is inferred from the body**, which the reference already promised
     (`fn(u: integer) -> integer`) and the implementation had never done — a fixture literally named
     `elided_definition_return_still_infers` was blessed at `-> Unknown`. A written `-> Unknown` is
     treated identically, because the reference says `Unknown` records "the checker could not tell"
     and is "not an explicit escape hatch" — `Any` is the way to say *do not check this*.

   Also folded in: `reconcile_declared_optionality` was a second, partial version of this
   reconciliation reachable only from the item root, so nested definitions never got it; it is gone,
   and its diagnostic now blames the function definition like its siblings instead of the whole
   assignment. A formal tested with `missing(x)` now counts as optional against the annotation too,
   which was a false positive on R's optional-without-default idiom.

   Seven fixtures. Findings byte-identical across data.table, dplyr, ggplot2 and shiny (7,116
   findings) — those packages carry no `#:` annotations, so that is a no-collateral-damage check, not
   coverage.


### E. Rendering drops information the message depends on


8. **FIXED — a rejected function now names the position that failed instead of printing both
   signatures.** As filed: `lapply(words, function(s) s + 1L)` over a character list reported
   ``expected `fn(character) -> T`, found `fn(s: U) -> U` ``, which describes a call that *should*
   fit and never mentions `character`, `+`, or numeric. Confirmed the underlying claim directly —
   `function(x) x + 1L` and `function(x) x` both render `fn(x: T) -> T` in a diagnostic, so an
   acceptable and an unacceptable function are indistinguishable.

   The premise was right but the diagnosis pointed at the renderer. A constraint belongs to the
   *variable*, not to the type: it can only appear in a binder prefix, and a diagnostic renders a
   monotype, so there is no place in `fn(s: U) -> U` for "U must be numeric" to go. Printing both
   signatures is therefore the wrong shape for this failure whatever the renderer does.

   What ships instead: `InferenceTable::explain_function_mismatch` re-walks the pairing and names the
   one position that failed, and the finding says what that position needs rather than showing a type
   it cannot show:

   - a parameter — *this function is passed `character`, but its parameter `s` is used as a numeric
     value (`integer` or `double`)*, or *…but its parameter `s` accepts `character`* when there is a
     type to show
   - the return — *this function must return `logical`, but its body produces a numeric value*

   Both signatures are still printed for a shape disagreement (arity, optionality, rest parameter),
   which is the case they genuinely explain, and a fixture pins that fallback.

   The pairing rule is now one function (`pair_parameters`) shared by the compatibility verdict and
   the explanation, so the two cannot drift.

   The filed alternative — pushing the expected parameter type into the lambda body — was **not**
   taken, and the reason is architectural: `CallArgument` inters each argument exactly once before
   any signature matching, so an overload probe can re-match without re-running expression inference.
   Bidirectional checking of a lambda argument would re-infer the body per candidate. Worth revisiting
   as its own slice if callback diagnostics need to point *inside* the lambda; the position-naming
   message covers the reported cases without it.

   Same root cause confirmed gone for the `Filter` report and the "expected `list[T] | T[]`, found
   `character[]`" one. Four fixtures. Across data.table/dplyr/ggplot2/shiny the finding *counts* are
   unchanged and exactly two messages differ, both real corpus findings that now read clearly.

9. **FIXED — a function member of a union renders with its grouping parentheses.** As filed:
   `(fn(A) -> B) | NULL` printed identically to `fn(A) -> (B | NULL)`, two different types, so
   copying a type out of a finding and back into an annotation silently changed it. The reference
   already specified the fixed behaviour — *"the optional callback is written `(fn() -> integer) |
   NULL`, which is also the form such a union renders as"* — the renderer simply joined members with
   ` | ` and never parenthesized.

   The rule is narrow and it is the only ambiguity in the grammar: `->` extends over a whole union,
   so a function *inside* a union needs its parentheses back, and nowhere else does. Everything else
   is delimited by a bracket, a comma, or a closing paren.

   It reached more than the filed case. The nullable-callback finding — *this may be `NULL` here, so
   calling it is not safe — its type is `fn(x: Any) -> character | NULL`* — was describing a function
   that returns a nullable character while talking about a value that may itself be `NULL`, and now
   reads `(fn(x: Any) -> character) | NULL`. A branch join of two function signatures printed
   `fn(shout: logical) -> fn(x: T) -> T | fn(x: U) -> character`, which under the `->`-extends rule
   is not the type meant at all. One fixture pins both spellings, written as the rendered forms
   pasted back, so it fails if either stops round-tripping.

10. **FIXED — a refused annotation reports once, and the report is the true one.** As filed, both
    halves reproduced (5 findings for the rank-2 case, not 7): the first was the false *"only one
    compact annotation fits in a `#:` block"* against a block holding exactly one, and the braceless
    `@param count integer` claimed a form clash that did not exist. Both prescribed a blank line that
    would not have helped. Three separate causes, and each is now closed:

    - **The form classifier counted parser-recovery debris as block items.** Recovery re-parents the
      pieces of a type it could not read to the block, so one line yielded three top-level types.
      The rules are about *lines* — every one of them says "separate with a blank line" — so the
      check now compares whole `#:` lines, a line's form being its first item's. A line carrying a
      second item did not parse as the form it committed to, which is a parse failure, not a clash.
    - **The refusal recovered by consuming the binder and stopping**, leaving the position with no
      type at all, so the enclosing parameter list then failed on the type that followed: one real
      finding under three consequences. It now reads the type as if the binder were absent.
    - **A refused block still carried its typing payload**, so the names the binder would have bound
      were reported as unknown types on top of the refusal. The parser marks a region it refused
      with an `ERROR` node, and a block containing one carries no payload and reports nothing of its
      own — the contract already said this for shape violations, and a parse failure is the same
      situation.

    Every rank-2 position now gives exactly one finding: parameter, return, list element, and
    directive payload. That last one was **silently accepted** before — `ann_braced_type` allowed
    binders, so `@param f {<T> fn(T) -> T}` parsed, bound nothing, and said nothing; a directive
    payload is not the outermost level (`@forall` and `@type Name<T>` are where the expanded and
    named forms declare parameters), so it is refused like any other nested binder.

    The code moved too, and the docs page that documented the old behaviour is updated: **the code
    says whose grammar was broken, not which stage noticed.** `syntax-error` is R the parser could
    not read; `annotation` is a `#:` comment that is wrong. Keying it on the stage put this
    deliberate limit under `syntax-error` while its siblings — an unknown constraint, a malformed
    block — reported as `annotation` only because lowering happened to catch them. `SyntaxError`
    already carried the `in_annotation` flag for exactly this distinction.

    Two traps worth keeping, both found by breaking the suite: an `ANNOTATION_MARKER` is **not**
    always a direct child of the block (a stitched line's marker lands inside whatever node was open
    across the break), so line boundaries must be counted over all tokens; and a `<T>` binder list is
    a *prefix* of the type after it, not an item, so counting it as one refuses every stub
    declaration that has a binder — which silently emptied the whole stdlib corpus.

    Noticed while fixturing, not fixed: the same cascade shape is visible in
    `annotations-types.R.test` for `fn([x] integer)` (4 findings) and `fn([1]: integer)` (9). Those
    are ordinary parser recovery rather than a deliberate refusal, so they need the recovery to
    resynchronise at the parameter boundary; worth a look but a different mechanism.

## test-user round 4: the syntax-error user (user directive)

One user breaking the parser on purpose across 17 cases — the shapes people actually produce
mid-edit — judging **direction** and **readability** as separate verdicts. Every number below is from
running the binary; R 4.3.3 was the referee wherever "is this even an error?" mattered.

**The scoreboard: 9 of 17 cases are exactly right** — one finding, on the token that broke it, worded
so an R user learns their own rule rather than the parser's state. Unclosed `{`, a stray `}`, an
unterminated string, a missing comma between arguments, `if ()` with no condition, and both
truncated-annotation cases (`fn(x: integer) ->`, `list{a: integer`) are all single precise findings.
A broken statement correctly silences the semantic findings around it: a function missing its `}`
mid-file produced *one* error and no noise from the three definitions after it. The model message is
the `else` one — ``unexpected `else`: it must stay on the same line as the `if` branch it belongs to
(or the `if` must be inside braces)`` — it explains R's rule and gives both fixes. That is the bar the
rest should meet.

### 1. FIXED — an unclosed opener put a false finding on EVERY following line

Recovery did not stop at the statement boundary: after reporting the unclosed `(`, the parser was
still inside the argument list, so each following line read as another argument.

```r
totals <- sum(c(1, 2, 3)      # the one real mistake
a <- mean(c(1))               # "missing `,` between these arguments"
b <- mean(c(2))               # "missing `,` between these arguments"
c <- mean(c(3))               # "missing `,` between these arguments"
```

**5 findings, 4 of them on code the user must not change**, scaling with the file — and the adopted
lines lost their own definitions too, so a later read of `a` came back `unresolved`. Now **one
finding**, on the unclosed `(`. Same for `[`.

**The rule is not "break at a line break", and two existing fixtures are why.** A list running onto
the next line is ordinary R, and a *fragment* there really is a forgotten separator — `sum(alpha\n
beta)` and `function(x\n y)` were both pinned reporting a missing comma, correctly, and a blanket
line rule regressed both to a worse pair of findings. What separates the cases is whether the next
line **assigns**: a line binding a name with `<-`/`<<-`, or opening with `if`/`for`/`while`/`repeat`,
is the next statement, and adopting it is what does the damage. A comma before it still makes it an
argument, so `run(1,\n  x <- 2)` stays silent.

**`=` was in that list at first, and that was wrong — it cost the commonest missing-comma case.**
Inside an argument or parameter list `name = value` is a *named argument*, which is exactly what a
multi-line call with a forgotten comma looks like. Counting it as a statement start gave

```r
config <- list(
  title = "Revenue"
  subtitle = "by quarter",
  width = 800
)
```

the unclosed-opener report instead of the one that names the missing `,`, then broke the rest of the
call into top-level statements — which drew two `assignment-operator` findings and an `unused` out of
source that had *not parsed*, contradicting the documented rule that a failed statement suppresses
findings overlapping it. **Six findings for one comma**, and the one the docs promised was gone. R
itself keeps consuming until the opener closes, so the named-argument reading is also the faithful
one. Pinned now by a fixture on that exact snippet; the three original cases all use `<-` and were
never affected, which is why nothing caught it.

Two more things were needed beyond the predicate. The newline is consumed while parsing the element
*before* the loop head, so the check has to scan backwards rather than watch trivia go by — a
peek-based version looked right and never fired. And once the list breaks, the statement loop still
saw a stray token where it wanted a newline and reported *again*, then recovered over the next line
and swallowed it anyway: an unterminated list now marks the statement, and the boundary check neither
re-reports nor recovers.

### 2. FIXED — a parse error pointed off the code, at zero width

```r
scale_by <- function(x,        # 3-line file
value <- 42
print(value)
```

The third finding was `expected a function body` at `4:1-4:1`. **The claim first filed here was
imprecise and is corrected:** with a trailing newline that line does exist (an editor counts the empty
position after the last `\n`), so it is not unplaceable — it is **zero characters wide**, underlining
nothing, on the blank line past the code, and pointing away from the construct that is actually
incomplete.

Two changes. The report now blames the `function` keyword, which is the construct that lacks a body —
eight characters wide, on the code. And it is not reported at all when the parameter list never
closed: that is the same mistake said twice, and the unclosed opener is the version worth reading.
Case 6 is down from 3 findings to 2, and the two that remain are exactly R's own reasoning (R also
reads `value` as a parameter, then chokes on `<-`).

### 3. FIXED — the annotation grammar reported every token it could not use

Measured before: `fn([x] integer)` → **4 findings**, `fn([1]: integer)` → **9**, one typo each, five of
the nine on the very same column. The type grammar cannot resynchronize the way the statement loop
can — a `#:` region is one expression with no boundary inside it to restart at — so it reported and
carried on.

Now **one finding per region**: the first thing it cannot use. Both cases report exactly once, and two
separate `#:` blocks still each report their own.

One refinement was needed and is worth keeping: an **unclosed opener** is discovered at the *end* of
the construct it names, so first-wins alone dropped the outermost truth in favour of an inner
consequence — `@type Point {list{x: double` reported only "expected `,` or `}` in this list type" and
never that the `@type` brace was open. One structural report of that kind now gets through regardless,
so that case reports both, outer first.

### 4. FIXED — cryptic where the tool already knew better

``unexpected character `“` `` for a typographic quote pasted out of a document: placement was perfect,
the wording was the parser's vocabulary. Measuring it found a whole family reported the same way —
curly quotes both kinds, en/em dash and the real minus sign, full-width parentheses and punctuation,
and the invisible ones.

The invisible ones were the worst: a non-breaking space printed as ``unexpected character ` ` ``, a
message pointing at what looks like ordinary whitespace and calling it unexpected.

Each now names what the character is and what to write instead — *`–` is an en dash, not R syntax —
use `-` instead* — and the invisible ones are **described rather than quoted**, because printing a
zero-width space between backticks shows the reader an empty pair and a caret over nothing. A
character with no known ASCII counterpart keeps the generic wording.

### 5. FIXED — a dangling binary operator swallowed the next statement, and the findings were useless

```r
total <- 1 +
label <- "next"
print(label)
```

No syntax error, and that is right: R parses it as `total <- ((1 + label) <- "next")` and fails at run
time. But the user got two ``I could not resolve `label` `` warnings on the lines that look correct,
and nothing anywhere about the dangling `+`.

Filed as wanting a lint; **the cause turned out to be sharper than that.** R refuses the shape
outright — `target of assignment expands to non-language object` — because the swallowed line became
the *target* of an assignment, and `1 + label` is not something you can assign to. `resolve_assignment_target`
had `_ => match self.replacement_base(target) { … None => self.resolve(target) }`, so an unassignable
target was silently read as an ordinary expression and its names reported.

Now reported where the mistake is, with the cause named when a trailing operator really is what
pulled the next line in: *this is a value, not a name, so nothing can be assigned to it — the operator
at the end of this line pulled the next line in as its right-hand side*. The target's own reads are
suppressed, so the two misleading warnings are gone (a genuine later read of the name still reports).

That clause is decided structurally, not by "the target spans a line": naming carries the span
*between the two operands* (which is where the operator lives) and the hint fires only when the line
break falls after it. A break before the operator (`a` newline `+ b`) is an ordinary continuation, and
a multi-line left operand says nothing about where the operator sits — both would have been described
wrongly by a plain contains-a-newline test.

Deliberately narrow, because R accepts more than a bare name: only a **computed value** and a
**non-string literal** are reported. `"x" <- 1` binds `x` in R and stays quiet; `` `y` <- 1 ``,
`names(v) <- …`, `attr(v, "k") <- …`, `v[[1]] <- …` all stay quiet; a *parenthesised* target is left
alone because R's own handling of `(x) <- 1` is odd enough that refusing it would be guessing. All
checked against R 4.3.3.

Two carve-outs the corpus forced, both worth remembering:

- **A `!`-headed target is exempt.** `!` binds tighter than `<-`, so `expr(!!name <- value)` — building
  an assignment rather than performing one — parses as an assignment to `!!name`. This was a real false
  positive in dplyr (`expr(!!obj_sym <- !!get_expr(wrapper))`, whose authors ship
  `utils::globalVariables("!<-")` to placate `R CMD check`). The refusal cannot tell it from a bare
  `!x <- 1` typo, and the metaprogramming form is common while the typo is not. Other unary operators
  are still reported.
- **The operand-gap span needs a bounds check.** Error recovery can leave the operands out of order,
  and `TextRange::new(lhs.end, rhs.start)` then panics. Caught by the `ide` fuzz suite, not by any
  fixture — which is the argument for the day-one fuzz rule in one line.

A "line ends in an operator" lint was measured and rejected instead: **15,129 lines across data.table,
dplyr, ggplot2 and shiny end in a continuation operator**, every one of them legal.

### FIXED — Found while doing that: a string assignment target created no resolvable binding

`"x" <- 1L` is legal R and binds `x` — verified against R, which prints `1`. Typing gave it the
binding (`x: integer`), but naming did not, so a later `print(x)` reported ``I could not resolve `x` ``:
a false positive on legal, if unusual, R.

The filed guess was that `replacement_base` should unwrap the string. **It should not, and that is the
part worth keeping.** The replacement path *reads* the base before writing it (`names(v) <- x` reads
`v`), and marks the write unreportable because a replacement is not a dead store. `"x" <- 1` is
neither: it is a plain definition, identical to `x <- 1`. So the string literal joins the name-target
arm by or-pattern instead — `ExpressionKind::NameRef(name) | ExpressionKind::Literal(LiteralKind::String(name))`
— which the lowering makes exact, because `string_value` has already stripped the quotes and resolved
escapes, so the payload *is* the name R binds. One pattern, and `<<-`, dotted names and the unused
check all follow from it: the dead store now reports at the literal's own range (`"x"`, quotes
included, which is what was written) instead of arriving from the item-level export path with the
whole statement underlined.

Backticks never needed this: `` `y` <- 1 `` lowers to a `NameRef` like any other name.

**The corpus caught a consequence, and it was a real bug — an older one than this change.** Binding
string targets took data.table from 4078 findings to 4079, and the extra one was a false `unused` on
`%fin%`, a helper operator data.table defines *inside* `merge` and calls twice on the next two lines.
The cause: a `%op%` read is recorded **by name** in `quiet_operator_reads`, which is what the
cross-item check for a package's own operator needs, but it never reached the slot model — so a
**local** operator definition had no read resolving to it and looked like a dead store. Backtick
definitions were already exposed to this; the string change only made data.table's case visible.

Fixed by extracting the slot half of a read (`mark_slot_read`) and calling it from the operator path
too, which leaves `resolve_read` as the only place that maps an expression id. **Three more
pre-existing false positives went with it**, each verified as a genuine use: `%+replace%` aliased from
`ggplot2::` inside `theme_custom` (three copies — the vignette `.R` plus two `.qmd`), and `%NA_OR%`
in shiny's `render-plot.R`, used four times below its definition. Corpus: data.table back to 4078
exactly, dplyr unchanged, ggplot2 1318 → 1315, shiny 1045 → 1044. An unused local operator is still
reported — marking the slot did not make the check blanket-quiet, and a fixture pins that.

### FIXED — the REPL wrapped a table that fits

`ry repl` never set R's `width` option, so R kept its default of **80 columns** whatever the terminal
was and `print()` wrapped a table with room on screen. The width is now measured once at startup and
applied with `R_ParseEvalString` in the window between `setup_Rmainloop()` and `run_Rmainloop()`.
Verified end to end in a pty: a 200-column terminal gives `width` 200, a 100-column one gives 100,
and an `.Rprofile` that sets 137 keeps 137 on a 200-column terminal — the profiles have been sourced
by then, so only R's untouched default is replaced.

**The first attempt fed `options(width = …)` through the ReadConsole hook, and it was wrong twice
over** — recorded because the console feed looks like the obvious mechanism and there is already a
`.Platform$GUI` fix using it. It costs a main-loop round trip, and a round trip taken before the
editor's first prompt leaves the terminal in the state R found it in, so the next read
desynchronises: two pre-existing pty tests began timing out and the suite went from 3.5 s to 94 s.
Queuing the statement earlier, alongside the other pre-prompt input, did not help — the round trip
itself is the problem, not its timing.

**Resize is not tracked, and closing that needs a different mechanism.** A stock terminal session
handles `SIGWINCH` by calling `R_SetOptionWidth`, which R does not export; the console feed is the
only other way in and is the thing that just proved unsafe between prompts. Worth revisiting if
someone finds a third route.
