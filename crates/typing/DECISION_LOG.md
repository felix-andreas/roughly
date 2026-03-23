# Typing Crate Decision Log

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
