# Typing Crate Decision Log

Keep newest decisions at the top.

- naming should split into file-local preparation and project-global resolution, and the project-global pass should assign distinct package-visible ids for top-level declarations while updating the same naming result built by the file-local pass.
  - Local lexical resolution and package-global resolution have different invalidation and tooling needs. The boundary should stay explicit, but it does not need a separate intermediate artifact. Remapping top-level declarations onto project-level ids still makes cross-file identity owned by the package result rather than by file-local traversal details.

- fixture runners return structured snapshots instead of pre-rendered joined strings.
  - The fixture suite now compares per-snapshot per-file outputs directly, carries expectations
    forward across generations, uses `#++++ any` for unchecked contents, and treats extra actual
    outputs as failures.

- parsed-document storage lives in `typing::workspace`, not in a separate `workspace` crate.
  - The package and script bucket model is now part of the typing phase boundary, and keeping it inside `typing` simplifies the API and removes stale cross-crate surface area.

- package-attached non-contributing files are `scripts`.
  - They are attached to a package, can resolve against the package namespace, and do not contribute back to that namespace.

- `lower` stays file-local, while `check.rs` owns package-scoped `run_lowering_and_naming` and full `check`.
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

- `parser` is not a real `typing` crate phase and should not remain on the public crate surface.
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
