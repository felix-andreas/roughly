# Decision Log

Keep newest decisions at the top.

- binding ids should stay document-local, and `run_naming` should preserve unchanged local naming results while rerunning only changed or missing documents.
  - Package-global binding ids drift under incremental recomputation and create the wrong dependency shape. Document-local ids keep binding ownership local, make the incremental boundary real, and let package lookup resolve through `global_bindings` plus the winning document's `global_exports`.

- project 005 should rebuild `global_bindings` package-wide from local export tables on each naming run, rather than trying to maintain that winner table incrementally now.
  - This keeps the implementation simple while naming is still package-wide, avoids repeated lazy package scans for cross-file lookups, and leaves a clean future upgrade path to a reverse `Symbol -> ordered exporters` index once package naming itself becomes incremental.

- local naming should use `non_locals` for value references outside file-local lexical resolution, and the type-side equivalent should move from `annotated_expressions` toward an explicit `referenced_type_names`-style work-item table.
  - `unresolved_values` is misleading because many of those names are only unresolved in the file-local pass, not after package-global lookup. `annotated_expressions` is too coarse because it records that an annotation exists, not the actual type-name references naming still needs to resolve.

- stable exported declaration identity is out of scope for project 005.
  - Project 005 only needs a cleaner naming data model and better incremental boundaries. Durable declaration identity matters later for tooling features across edits, not for this naming cleanup.

- package-global non-local lookup should not be eagerly materialized as `ExpressionKey -> BindingId`; it should stay symbol-keyed until a later consumer resolves through package exports.
  - Eager materialization ties naming results to snapshot-local binding ids and makes incremental reuse worse. The symbol-keyed package export table is the more stable boundary for project 005.

- duplicate top-level value names should warn only when they conflict in package-visible naming; non-package documents may reuse those names without the package-global duplicate-binding warning.
  - Package files contribute to one package-global value namespace, so conflicts there are meaningful and should warn. Non-package documents do not contribute to that namespace, so their top-level rebinding should stay script-local.

- naming should use one local `BindingId` space directly and drop `ProvisionalBindingId`, and package-global symbol tables should point to defining modules without redundantly storing module-local binding ids.
  - Provisional ids were a migration seam, not a semantic requirement. Keeping package tables symbol-keyed plus module-keyed improves incremental recomputation boundaries while module-local `global_exports` remains the source of concrete local binding ids.

- non-package documents can resolve package-global value and type names, but they do not contribute
  back to those namespaces or conflict with package files on same-name declarations.
  - This matches the script-file naming contract now captured in `TYPING_SEMANTICS.md` and the naming
    fixtures: package-attached scripts are consumers of package-global namespaces, not producers.

- `run_naming` should consume lowered package state rather than triggering lowering itself.
  - The phase boundary should be real so tests, tooling, and incremental scheduling can call lowering and naming separately. `run_naming` itself should not hide lowering work behind a combined wrapper.

- package-global value resolution uses one final symbol table built from top-level exports, and the file-local naming pass does not resolve globals even within the same file.
  - This matches the intended runtime-like package semantics better than preserving earlier top-level bindings at individual use sites, and it gives naming a cleaner split between file-local lexical facts and package-global consolidation. Duplicate top-level value definitions still warn on both the overwritten and overwriting declarations.

- `workspace` should stay thinner than `package` and should not mirror package mutation APIs.
  - `Package` is the analysis unit and should own package contents directly. `Workspace` is the editor-facing registry and mutation helper around packages plus detached scripts, not a second package API surface.

- fixture runners return structured snapshots instead of pre-rendered joined strings.
  - The fixture suite now compares per-snapshot per-file outputs directly, carries expectations
    forward across generations, uses `#++++ any` for unchecked contents, and treats extra actual
    outputs as failures.

- parsed-document storage lives in `analysis::workspace`, not in a separate `workspace` crate.
  - The package and script bucket model is now part of the analysis phase boundary, and keeping it inside `analysis` simplifies the API and removes stale cross-crate surface area.

- package-attached non-contributing files are `scripts`.
  - They are attached to a package, can resolve against the package namespace, and do not contribute back to that namespace.

- `lower` stays file-local, while `analysis.rs` owns package-scoped phase orchestration and full `check`.
  - The package is the unit of naming and later semantic phases, but lowering still needs to run directly on individual parsed documents.

- `type_syntax` simple fixtures compare the parser's rendered success or rendered failure directly, with no `error:` sentinel in fixture input.
  - The fixture body should contain only the actual source under test. The runner always calls `parse_type_syntax` on that source and compares either the rendered type or the rendered parse error.

- the fixture crate parses the fixture language, and the testing framework combines that parsed data with `workspace` state.
  - Expectations attach to the immediately preceding file or operation. If a document already has an expectation from an earlier generation, that expectation carries forward until replaced, deleted, or moved.

- type names are project-global, and top-level value names are package-global across files.
  - `@type` and `@alias` declarations share one project-global namespace, forward references are allowed across files, and duplicate type names are errors at every conflicting declaration. Top-level value names are visible across package files, later files in package collation order win on conflicts, and both overwritten and overwriting value definitions should warn. We also want to leave room for a future file-local opaque-type feature.

- naming data is a tooling boundary as well as a typechecking boundary.
  - The naming result should support go-to-definition and local rename within a file now, and it should scale to project-level rename once cross-file naming data exists.

- HIR stores top-level type declarations separately from top-level executable expressions.
  - Type declarations are not expression nodes, and their interleaving with executable top-level expressions is not semantically significant for later phases.

- non-top-level `@type` and `@alias` definition blocks are lowering errors.
  - Declaration placement is a structural front-end concern, so the checker should reject it before naming or typechecking.

- `@new` accepts nominal type references, including generic nominal applications.
  - This keeps nominal introduction aligned with ordinary nominal type syntax while still rejecting aliases and structural type forms; generic nominal types therefore require full type arguments such as `@new Person<integer>`.

- assignment annotations live only on the assignment expression, not in a second `Assign.annotation` slot.
  - `AttachedAnnotation` now uses explicit variants for expression-only versus binding-and-expression attachment, so duplicating assignment annotations in HIR was redundant.

- The parser module for `#:` typing blocks is `type_syntax`.
  - It handles both attachable annotations and standalone `@type` / `@alias` definition blocks, so `annotations` was too narrow.

- The public parser entrypoint is `parse_type_syntax`, and its result type is `TypeSyntax`.
  - This removes the old naming overlap where the public parser used `annotation` for both the whole parsed block and one semantic variant.

- `check` remains the top-level orchestration entry point.
  - This keeps the public entry point aligned with the full checking pipeline instead of exposing internal phases.

- `parser` is not a real `analysis` crate phase and should not remain on the public crate surface.
  - Syntax parsing is external integration or test support, while the checker should be able to consume already-parsed syntax.

- `naming` is the agreed phase name.
  - The phase resolves names to stable identities, but `naming` is the agreed architectural term.

- `typecheck` is the semantic checking phase, and inference is an internal mechanism inside it.
  - This matches the intended phase boundary better than treating HM inference as the phase itself.

- diagnostics are not a top-level pipeline phase.
  - Diagnostics are output produced by lowering, naming, and typechecking rather than a separate execution stage.

- annotation parsing should happen during lowering exactly once.
  - This avoids duplicate work and gives lowering a clean annotated front-end boundary.

- `hir.rs` and `lower.rs` should be separate files.
  - Keeping the representation separate from the lowering logic makes the phase boundary clearer.

- naming should stay distinct from lowering.
  - This preserves a clean front-end boundary even if the implementation runs both phases back to back.

- `diagnostic.rs` should exist as a shared module for structured diagnostics and rendering.
  - Keeping diagnostic data, codes, severities, ranges, and rendering together makes wording changes and multi-phase reporting easier than rendering directly at error creation sites.

- HIR should use arena or id-based storage.
  - Stable ids make naming tables, type tables, hover lookups, and later incremental work fit the architecture better than nested tree-only storage.

- keep one `typecheck.rs` for now.
  - Splitting out a separate engine file can wait until the internal structure is clearer.

- keep builtin typing, compatibility logic, and interface extraction inside `typecheck.rs` for now.
  - Those splits should wait until the typechecking structure stabilizes.

- successful-check fixtures should be split into separate suites rather than one overloaded suite.
  - Different checked outputs have different contracts and should not be forced into a single renderer.

- use `expressions` as the suite name for smaller checked-expression cases.
  - This matches the current `inference` suite purpose more closely than `typecheck`.

- Naming produces side tables keyed by stable ids.
  - Now that HIR uses an arena and `ExpressionId`, side tables prevent allocating an entire new `NamedFile` tree. It also simplifies mapping AST locations for hover tools.

- Naming resolves only value names for now.
  - Since types are currently represented by `SurfaceType` from annotations, type-level scoping isn't necessary yet.
