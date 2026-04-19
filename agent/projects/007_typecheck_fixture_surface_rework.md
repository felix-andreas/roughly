# Typecheck Fixture Surface Rework [planning]

## Goal

Realign current typecheck fixture surface with `agent/TYPING_SEMANTICS.md` so user-facing semantics are primary contract and inference-mechanism details are secondary helper coverage.

The target is an excellent semantics-first suite, not a taxonomy that merely reflects current
implementation seams or current passing coverage.

This project covers only `crates/analysis/tests/typecheck/`.
It does not include `type_syntax`.
Keep the dedicated diagnostics fixture suite for now.
If a future incremental-analysis suite becomes the owner of full rendered diagnostics, that will be a later follow-up rather than part of this refactor.

## Current shape

- Typecheck fixtures currently live under `crates/analysis/tests/typecheck/`.
- Suite split is mostly engine-centric: `bindings`, `environment`, `expressions`, `generalization`, `instantiation`, `interfaces`, `substitution`, `unification`.
- Harness support is misleadingly granular:
  - `bindings` and `generalization` use the same runner shape and output contract.
  - `environment`, `instantiation`, and `substitution` use the same runner shape and output contract.
  - `expressions` and `unification` use the same runner shape and output contract, differing mostly in whether cases leave inference variables unsolved.
- Those older engine-centric suites now live under `crates/analysis/tests/typecheck/deprecated/`
  so they remain available as migration storage without pretending to be part of the target split.
- The first migration slice is now implemented:
  - unique binding-boundary coverage from deprecated `generalization` is mirrored into `bindings`
  - unique use-site coverage from deprecated `environment`, `instantiation`, and `substitution` is mirrored into `expressions`
  - deprecated fixture files still remain temporarily as migration backstop, but they are no longer
    wired as active suites
- Most typecheck suites still run only `Simple` fixtures, so typecheck cannot directly cover multi-file package semantics yet.

## Mismatch with semantics contract

`agent/TYPING_SEMANTICS.md` organizes semantics by:

- typing comment syntax
- naming and scoping
- type annotations and assertions
- types
- operators
- loops
- function types
- unsupported constructs

The current typecheck suite names do not mirror those contracts. They instead expose implementation stages like generalization, instantiation, substitution, and unification that are not user-facing semantic categories.

## Coverage gaps

- Multi-file typecheck coverage is missing for package-global value and type behavior.
- `Collate` and file-order-sensitive typecheck behavior is not covered.
- Non-package document typecheck behavior is not covered in typecheck fixtures.
- `@new` nominal introduction currently appears only in diagnostics coverage.
- Generic named types and `@forall` function annotations lack dedicated typecheck fixture coverage.
- Some type-definition and nominal behavior appears only in diagnostics fixtures, so the happy-path semantic surface is under-specified.

## Implementation blocker from next slice

The next fixture slice exposed a real type-model gap, not merely missing tests.

New fixtures added in this slice currently fail in three clusters:

- expanded `@forall` binding annotations
  - expected binding scheme: `identity: <T> fn(value: T) -> T`
  - actual binding scheme: `identity: fn(value: Unknown) -> Unknown`
- alias annotations on bindings
  - checked alias annotations currently fail with `error: type mismatch`
- nominal introduction and exported nominal values
  - expected binding/interface value surface: `Person`, `Person<integer>`
  - actual value surface stays structural: `list{name: character}`, `list{value: integer}`

Root cause verified in implementation:

- `SurfaceType::Named(_, _)` is erased to `CoreType::Unknown` in `core_type_from_surface_type`
- `SurfaceType::Binders(_, inner)` is erased to `inner`
- `Annotation::New { .. }` returns structural inferred type unchanged in `apply_annotation`
- interface fixture rendering already has two different views:
  - type definitions render from HIR `module.definitions`
  - exported values render from `TypeScheme`
  - so definitions can still print `type Person = ...` while values lose nominal identity and print only structural type

This means current typecheck has no way to preserve named-type identity or explicit quantifier intent once annotations enter core typing.

### Design review

Current source of truth:

- HIR type definitions and annotations preserve `SurfaceType::Named(...)`, `SurfaceType::Binders(...)`, and `@new`
- naming validates those references against package-visible definitions

Structurally weak part:

- typecheck lowers away that semantic information immediately
- fixture and interface surfaces then depend on two inconsistent representations:
  - definitions still know named type identity
  - value schemes do not

Simpler target shape:

- add explicit named user-defined type form to core typing, so value schemes can preserve named identity
- keep type definitions themselves as source of truth for bodies and kind (`@type` vs `@alias`)
- lower checked/trusted/unknown-only named annotations and `@new` into that named core form instead of erasing to `Unknown` or structural shape
- keep explicit quantified binders alive through annotation lowering, so `@forall` contributes directly to binding schemes instead of being reconstructed indirectly from inference variables

Likely first implementation direction:

- extend `CoreType` with named type variant carrying stable identity plus core-type arguments
  - current best candidate is package-unique type `Symbol` plus instantiated core arguments
  - naming already rejects duplicate package-visible type names, so symbol can act as stable identity in current model
- teach compatibility and unification how named types interact with structural bodies
  - aliases should be structurally compatible with their expansion
  - nominals should preserve identity while still allowing explicit projection to underlying structural type where semantics allow it
- teach `apply_annotation` that `@new Person` constructs nominal `Person`
- teach annotation lowering to preserve binders instead of dropping them before binding generalization

Expected impact:

- correctness
  - typecheck fixtures can express nominal, alias, and explicit-polymorphism semantics honestly
- simplicity
  - one typed representation for exported values instead of structural value hacks plus separate definition rendering
- performance
  - slightly richer core type, but still single-source-of-truth and incremental-friendly
- incremental analysis
  - named identity survives into stored schemes, which is required for future cross-file/project typing anyway

## Current thin areas

The suite is thin in a few high-value places even after the first migration slice.

- `bindings` remains small relative to the semantics contract.
  - current target suite coverage is still mostly basic values, basic functions, and a few function annotations
  - it needs much more around checked annotations, `@trust`, `@if-unknown`, `@new`, aliases, nominals, generics, and `@forall`
- `interfaces` is especially thin.
  - it currently proves only a few exported-value and exported-type snapshots
  - it needs nominal exports, generic exports, richer mixed value-plus-type surfaces, and more rebinding/export order cases
- `expressions` has decent breadth but still misses important depth.
  - many areas have only one or two representative cases rather than an explicit boundary matrix
  - nominal and generic type usage is especially under-covered
  - function annotation success coverage is still thin relative to the semantics document
- there is still no direct typecheck coverage for project semantics.
  - cross-file value use
  - cross-file type use
  - package winner behavior
  - non-package consumer behavior
- diagnostics still carry too much type-system contract.
  - some nominal and type-definition success or failure behavior appears there instead of in semantic typecheck suites

Counts are not the main issue, but the current target suites make the imbalance visible:

- `bindings`: small
- `interfaces`: very small
- `expressions`: much larger, but still missing several semantics families

## Concrete backlog

### `typecheck/bindings`

#### `annotations.R.test` [pending]

- add checked annotation happy paths beyond the current compact function default-return case
  - scalar checked annotation
  - vector checked annotation
  - list checked annotation
  - nullable-union checked annotation
- add binding-boundary coercion cases
  - `@trust` accepts incompatible source and stores requested type
  - `@if-unknown` accepts `Unknown`
  - `@if-unknown` rejects known source type in diagnostics, while success coverage stays here
- add named-type annotation success cases
  - alias annotation on binding
  - nominal annotation after successful `@new`

#### `functions.R.test` [pending]

- expand function-binding scheme matrix
  - explicit `@forall` compact annotation
  - explicit `@forall` expanded annotation
  - higher-order annotated bindings with named parameters
  - optional positional versus optional named parameter shapes
  - higher-order binding with nullable return

#### `nominals.R.test` [pending, new]

- add nominal and alias binding-boundary success coverage
  - alias-backed binding stores aliased structural type
  - `@new` nominal introduction stores nominal type
  - generic alias binding
  - generic nominal binding
  - nominal value assignable to underlying structural type at later binding boundary

### `typecheck/expressions`

#### `functions.R.test` [pending]

- expand function-annotation and call boundary matrix
  - explicit `@forall` success cases
  - compact anonymous parameter versus named parameter call behavior
  - optional parameter omitted and supplied
  - nullable parameter and nullable return cases
  - higher-order annotation boundary cases

#### `special_types.R.test` [pending]

- add more binding/use-site interplay for special types
  - checked annotation accepts `NULL | T` from `NULL`
  - checked annotation accepts `NULL | T` from `T`
  - `Any` through calls and higher-order positions
  - `Unknown` propagation through calls where callee or return is unknown

#### `control_flow.R.test` [pending]

- expand branch and loop boundary matrix
  - more `if` normalization cases where branch type is already `NULL`
  - `for` over record-like list
  - `for` over map-like list annotation
  - loop body using iterated element type in typed expression

#### `indexing.R.test` [pending]

- add unsupported and boundary indexing cases
  - `[` on vectors remains unsupported
  - map-like vector `[[` miss still nullable
  - tuple/record `[[` boundary literal positions and names
  - backtick-quoted `$` name sugar

#### `nominals.R.test` [pending, new]

- add use-site success coverage for named types
  - alias use inside larger expressions
  - nominal use in function parameter and return positions
  - successful `@new` then use through nominally typed function
  - generic alias use
  - generic nominal use

#### `polymorphism.R.test` [pending]

- expand polymorphism depth, not only representative cases
  - explicit `@forall` use-site behavior
  - alias of higher-order polymorphic binding
  - returned polymorphic function reuse
  - polymorphic nullable return

### `typecheck/interfaces`

#### `types.R.test` [pending]

- expand exported type-definition matrix
  - nominal exports
  - generic alias exports
  - generic nominal exports
  - mixed alias plus nominal plus value surface

#### `functions.R.test` [pending]

- expand exported binding surface
  - annotated monomorphic exports
  - higher-order exports
  - latest-binding-wins with richer function shapes
  - exported binding order with interleaved type definitions

#### `nominals.R.test` [pending, new]

- add exported nominal surface explicitly
  - exported nominal plus constructor-introduced value
  - exported generic nominal plus value

### `typecheck/project`

#### `values.R.test` [pending, new]

- cross-file value use
- later-file winner behavior
- non-package consumer reading package-global values

#### `types.R.test` [pending, new]

- cross-file alias use
- cross-file nominal use
- cross-file `@new`
- package file and non-package file reuse of same type name without conflict

#### `scripts.R.test` [pending, new]

- non-package script sees package globals
- script-local top-level bindings do not become package-global
- script-local type declarations do not become project-global

#### `collate.R.test` [blocked, new]

- file-order-sensitive typecheck once fixture harness can model `DESCRIPTION`

### `diagnostics`

#### `types.R.test` [pending]

- keep syntax and typing-comment rendering here
- keep nominal-introduction rejection and top-level-only definition placement here
- move semantic success coverage out to typecheck suites instead of growing more happy-path facts here

#### `special_types.R.test` [pending]

- keep user-facing wording/range/code checks here
- avoid using this file as only home for nullable-union or `Any` semantics

## Recommended next slices

1. Settle and implement named-type and explicit-binder preservation in typecheck so new nominal/alias/`@forall` fixtures have honest backing semantics.
2. Finish current slice after that support exists:
   - expand `bindings/annotations.R.test`
   - add `bindings/nominals.R.test`
   - expand `interfaces/types.R.test`
3. Add `expressions/nominals.R.test` plus `@forall` cases in `bindings/functions.R.test` and `expressions/functions.R.test`.
4. Introduce `typecheck/project/` with `values.R.test`, `types.R.test`, and `scripts.R.test`.
5. Once target suites cover those semantics, start deleting duplicated deprecated files instead of keeping them indefinitely.

## Proposed direction

The first draft above was too literal. One suite per semantics chapter would over-split the fixture tree and create overlap.

Weak points in that draft:

- `annotations` cuts across bindings, expressions, functions, and nominal introduction.
- `operators` becomes grab-bag huge.
- `loops` is probably too small to justify a top-level suite.
- `scoping` risks duplicating naming as a second source of truth.
- `interfaces` is not a language-semantics chapter; it is separate because its rendered output contract is different.

Better direction: top-level split by rendered output contract, semantic split inside those suites.

Recommended top-level suites:

- `typecheck/expressions`
  - renders resolved expression result types or normalized error kinds
- `typecheck/bindings`
  - renders generalized binding schemes at binding boundaries
- `typecheck/interfaces`
  - renders exported surface per file or later per package snapshot
- `typecheck/project`
  - multi-file and package-visible typed behavior once multi-file support lands
- optional `typecheck/unification` or `typecheck/internal`
  - raw metavariable-facing engine coverage if fixture-level internal coverage remains useful

Keep dedicated `diagnostics` alongside this split for now.

### Suite boundaries

- `bindings`
  - asks what type scheme gets stored at a binding boundary
  - output shape is `name: TYPE_SCHEME`
  - may show rebinding history
  - should not collapse to only final exported surface
- `expressions`
  - asks what type an expression evaluates to at a use site
  - output shape is resolved expression result types or normalized error kinds
  - should carry most ordinary language semantics such as calls, control flow, indexing, polymorphic use, and scoping effects
- `interfaces`
  - asks what surface a file exports after internal rebinding settles
  - output shape is final exported bindings plus exported `@type` and `@alias`
  - may collapse repeated top-level rebindings to the final visible export

These are genuinely different output contracts, so they should remain separate suites even when they exercise related semantics.

Recommended semantic file grouping inside those suites:

- `vectors`
- `lists`
- `special_types`
- `nullable_unions`
- `function_annotations`
- `function_calls`
- `polymorphism`
- `control_flow`
- `indexing`
- `arithmetic`
- `scoping`
- `nominals`
- `aliases`
- `unsupported`

Likely placement:

- `@trust`, `@if-unknown`, checked annotations:
  - expression-level cases in `expressions/*`
  - binding-boundary cases in `bindings/*`
- `@new`, aliases, nominal success cases:
  - `bindings/nominals` and `project/nominals`
- `@forall`, compact and expanded function annotations:
  - `bindings/function_annotations`
  - `expressions/function_calls`
- loops:
  - probably stay in `expressions/control_flow` unless they grow enough to justify their own file

Ownership boundary:

- naming fixtures own resolution identities
- typecheck fixtures own typed consequences of those resolved identities
- future incremental-analysis coverage may own full rendered diagnostics across phases

That keeps one source of truth for each semantic fact and avoids mirroring the same cross-file scenario in several suites.

Keep internal mechanism coverage separate and smaller:

- low-level `InferenceState` invariants stay in direct Rust tests
- if fixture coverage is still needed for raw engine shapes, keep one clearly-internal suite such as `typecheck/unification` or `typecheck/internal`

## Naming direction

- Prefer suite names from semantics contract, not implementation steps.
- Prefer fixture group names from semantic topic, for example `nullable_unions`, `named_parameters`, `nominal_types`, `cross_file_exports`.
- Prefer case names that state rule being exercised, for example `if_without_else_returns_nullable_branch_type`.
- Keep `group__case` stable through moves so focused test filters survive suite refactors.

## Documentation direction

- `agent/TESTING.md` should describe:
  - `typecheck/expressions`
  - `typecheck/bindings`
  - `typecheck/interfaces`
  - dedicated `diagnostics`
  - optional internal `typecheck/unification`
- Each fixture suite should have a local `README.md` with:
  - purpose
  - rendered output contract
  - coverage matrix
  - explicit current gaps until the suite reaches the intended matrix
  - what does not belong in the suite
  - naming guidance for groups and cases
- Preferred suite-local READMEs:
  - `crates/analysis/tests/typecheck/README.md`
  - `crates/analysis/tests/typecheck/bindings/README.md`
  - `crates/analysis/tests/typecheck/expressions/README.md`
  - `crates/analysis/tests/typecheck/interfaces/README.md`
  - `crates/analysis/tests/typecheck/unification/README.md` if retained
  - `crates/analysis/tests/diagnostics/README.md`

## Suggested migration order

### 1. Update testing docs first

- update `agent/TESTING.md` to reflect the target split and suite contracts
- add suite-local `README.md` files with coverage matrices before moving fixtures

### 2. Collapse duplicate runner contracts

- Replace near-identical runner functions with shared runner modes.
- Make directory split reflect output contract first, not engine vocabulary.

### 3. Re-home existing cases under contract-first suites

- Keep only a few top-level runner contracts.
- Move most current `expressions/*` files into semantic files under `typecheck/expressions`.
- Fold `generalization` into `bindings`.
- Fold `instantiation`, `substitution`, and most of `environment` into semantic files under `expressions` or `bindings` depending on rendered output.
- Keep `interfaces` separate because it has a genuinely different output contract.
- Concrete current-file mapping:
  - `bindings/annotations.R.test` + `deprecated/generalization/annotations.R.test` -> `bindings/annotations.R.test`
  - `bindings/basics.R.test` + `deprecated/generalization/basics.R.test` -> `bindings/basics.R.test`
  - `bindings/functions.R.test` + `deprecated/generalization/functions.R.test` -> `bindings/functions.R.test`
  - `deprecated/environment/functions.R.test` -> `expressions/scoping.R.test` and `expressions/polymorphism.R.test`
  - `deprecated/environment/scoping.R.test` -> `expressions/scoping.R.test`
  - `deprecated/instantiation/basics.R.test` -> `expressions/functions.R.test`
  - `deprecated/instantiation/functions.R.test` -> `expressions/polymorphism.R.test`
  - `deprecated/substitution/basics.R.test` -> `expressions/functions.R.test` or `expressions/polymorphism.R.test`
  - `deprecated/substitution/functions.R.test` -> `expressions/polymorphism.R.test`
  - `unification/basics.R.test` + `unification/functions.R.test` -> keep only if raw metavariable fixture coverage still desired

### 4. Add missing happy-path semantic coverage

- add `@new`
- add generic named types
- add `@forall`
- add more alias and nominal success cases

### 5. Add multi-file typecheck fixtures

- cross-file value use
- cross-file type use
- package file winner behavior
- script consumer behavior
- later `Collate` coverage when harness supports `DESCRIPTION`
- keep these under `typecheck/project` so package-visible semantics have one obvious home

### 6. Revisit diagnostics only after incremental-analysis suite exists

- do not use diagnostics fixtures as substitute for missing semantic type-output coverage
- keep dedicated diagnostics coverage until a real incremental-analysis suite can absorb that contract

## Open questions

- Should `unification` survive as one explicit internal fixture suite, or should all remaining engine-detail coverage move into direct Rust tests?
- Should typecheck multi-file fixtures render per-file typed snapshots, or assert only selected files per case?
- Should `typecheck/project` render full per-file typed snapshots or a narrower package-facing summary?
