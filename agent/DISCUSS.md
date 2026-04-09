# Discussion

## Open decisions

- Should the checker keep a separate naming phase, or collapse naming into typecheck entirely?
- Should unresolved names and unknown types be treated the same by later phases?

## Active discussion

### Typecheck should use naming outputs

Status:
- implemented for package/global value resolution

Current source of truth:
- `naming` already resolves local symbol uses into `expression_resolutions`, tracks package-facing exports in `global_exports`, and records file-local misses in `non_locals`.
- `typecheck` still does its own environment lookup by raw `Symbol`, so it is effectively performing a second name-resolution pass.

Current structural problem:
- This duplicates resolution logic across `naming.rs` and `typecheck.rs`.
- It produces duplicate diagnostics when naming reports an unresolved symbol and inference later reports its own unknown-name failure.
- The merged-module typecheck path in `analysis.rs` also weakens the boundary by discarding the original per-document naming context before inference runs.

Simpler target shape:
- `naming` remains the only name-resolution phase.
- `typecheck` consumes `NamesLocal` and `NamesGlobal` instead of resolving by symbol names on its own.
- Symbol expressions should be interpreted through naming facts:
  - local references from `expression_resolutions`
  - package/global references from `non_locals` plus `NamesGlobal` and the exporting document's `global_exports`
- If naming already left a symbol unresolved, typecheck should treat that use as `Unknown` and avoid emitting a second unresolved-name diagnostic.

Likely implementation direction:
- Stop relying on merged-module symbol lookup as the resolution boundary.
- Keep one shared inference state for package checking, but drive lookup through naming identities rather than raw symbols.
- Use existing naming data first and only then infer types for resolved bindings/usages.

Expected impact:
- correctness: one source of truth for name resolution
- diagnostics: no duplicate unresolved-name reports
- simplicity: typecheck stops re-implementing naming behavior
- incremental analysis: better fit with the existing per-document naming caches

### Separate naming phase vs typecheck-only

Question:
- Is a separate name-resolution phase common, and is there a good reason for it?
- Would it be simpler to have only a typecheck phase?

Short answer:
- Yes, a separate naming or resolution phase is common.
- In this codebase, removing the separate naming phase would very likely make things worse, not simpler.

Why separate naming phases are common:
- Name resolution and type inference solve different problems.
- Tooling features such as hover, go-to-definition, rename, and duplicate-definition checks need binding identity even when typechecking is disabled or incomplete.
- Resolution facts are often reusable across later phases, while typechecking is more expensive and more sensitive to incomplete code.

Why collapsing into typecheck would not be simpler here:
- The project already has naming data as a real source of truth used by diagnostics and IDE behavior.
- If typecheck absorbs naming, it would need to reproduce all of those binding tables anyway, or tooling would need to depend on typecheck internals.
- That would either duplicate state or make type inference own too many unrelated responsibilities.
- It would also make fast editor paths worse, because simple features and early diagnostics would depend on running deeper semantic work.

Recommended shape:
- Keep a distinct naming phase.
- Make naming the only resolution pass.
- Make typecheck consume naming outputs instead of resolving names again.

This keeps the phase split for real semantic reasons rather than ceremony:
- naming answers "what does this reference mean?"
- typecheck answers "given those bindings, what types are valid here?"

### Unresolved vs unknown in later phases

Recommendation:
- Keep the diagnostics distinct at the naming layer.
- Treat both as `Unknown` for later semantic checking so typecheck does not cascade.

Why keep them distinct diagnostically:
- An unresolved value name and an unknown type name are different user mistakes.
- They should keep different wording and different source locations when naming reports them.

Why treat them the same in typecheck:
- After naming has already reported the root problem, typecheck should not try to recover by inventing more lookup failures.
- Both cases mean "the semantic input here is missing", so the most useful recovery value is the same: an unknown type placeholder that lets checking continue without a large error cascade.

Recommended contract:
- naming emits the primary unresolved/unknown diagnostic
- typecheck consumes that missing semantic fact as `Unknown`
- typecheck still reports real downstream contradictions when they are directly informative, but it should not emit a second unresolved-name style diagnostic for the same cause

This means the distinction survives in the user-facing diagnostic model, but not as a separate control path inside inference.
