# Discussion

This file is scratch space for active design discussion.

Use it for short summaries or temporary notes during a design conversation.

## Roughly diagnostics migration

### Current shape

- `roughly` still owns three frontend diagnostic families:
  - syntax
  - linting (`T`/`F`, `=` assignment, naming style, trailing commas)
  - unused-variable checks
- `analysis` already treats diagnostics as phase outputs rather than a separate pipeline stage.
- `analysis::lower` already produces:
  - parser/tree structural syntax diagnostics
  - typing-comment syntax diagnostics
- `analysis::naming` already produces semantic name-resolution diagnostics.

### Structural issue

- Diagnostics currently have two owners for the same document state:
  - `roughly` for frontend syntax/linting
  - `analysis` for lowering, naming, and typing
- Current source of truth is therefore split by diagnostic family rather than by phase ownership.
- The simpler target shape is:
  - `analysis` owns all diagnostic production
  - `roughly` owns config, scheduling, and LSP conversion only

### Recommended target shape

- Remove unused-variable diagnostics instead of migrating them. (yes)
- Keep syntax diagnostics owned by `analysis::lower`. (yes)
- Add a new file-local `linting` phase inside `analysis` for non-semantic linting.
- Run that phase alongside lowering and before naming, and store its output as phase diagnostics in `Analysis`.
- Do not add a durable lint artifact or cache as a second source of truth; keep only diagnostics. (yes)

### Why not put linting into naming

- The current lint rules do not depend on name resolution or package state.
- Putting them in naming would mix semantic and non-semantic responsibilities.
- Linting should still run even when naming/typecheck are blocked by syntax or lowering failures.
- The implementation wants the same inputs as lowering: parsed tree plus source text.

### Separate `linting` phase vs folding linting into lowering

Recommendation: keep `linting` separate from `lower`.

Reasons:

- Lowering should remain the structural syntax-to-HIR boundary.
- The existing lint rules are syntax-tree walks and do not need HIR.
- Folding linting into lowering would make lowering own two unrelated responsibilities:
  - construct HIR
  - enforce style conventions
- A separate file-local phase keeps the contracts cleaner without introducing duplicated state, because the phase emits diagnostics only.
- Testing also becomes cleaner:
  - lowering fixtures stay about HIR and lowering-owned failures
  - linting fixtures stay about style diagnostics

Cost:

- one more phase enum and a bit more orchestration plumbing in `Analysis`

That cost is worth paying because it preserves a simpler architecture boundary.

### Proposed phase placement

- parse outside `analysis`
- `lower`
  - structural syntax diagnostics
  - typing-comment parsing and diagnostics
  - HIR construction
- `linting`
  - tree-local lint diagnostics
- `naming`
- `typecheck`

This keeps the architectural rule that diagnostics are outputs of real phases, not a separate render step.

### Settled points

- Keep naming-style linting configurable.
- `roughly` should pass lint config into `analysis`; `roughly` should not filter already-produced diagnostics after conversion.
- Run syntax and lint diagnostics from `analysis` on every `did_change`.
- Keep the old unused-related config accepted for backward compatibility, but make it a no-op.

### Open decisions

- Whether the phase should be named `linting` or `lint`

## Syntax lowering tests

### Resolution

- Tree-sitter R does not currently produce `for_statement`, `if_statement`, or `while_statement`
  nodes with a missing `close` field for malformed missing-`)` control-flow heads.
- The reachable parse shape is a top-level `ERROR` node whose first children are the control-flow
  keyword and `(`.
- Lowering now handles that reachable `ERROR` shape directly and emits `missing closing delimiter )`
  for `for`, `if`, and `while` heads.
- The dead statement-node branches were removed from lowering, and `syntax.R.test` now covers the
  real parser shapes for those cases.
