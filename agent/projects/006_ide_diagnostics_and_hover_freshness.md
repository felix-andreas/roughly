[planning] IDE Diagnostics And Hover Freshness

Concept phase only. Not ready to be implemented.

## Unresolved questions

- Should hover always use current unsaved-buffer state, or is any saved-snapshot fallback acceptable?
- On `did_change`, should the editor show:
  - only current front-end diagnostics
  - current front-end diagnostics plus last published semantic diagnostics
  - or some other merged policy?
- What is the smallest state model that cleanly separates:
  - semantic recomputation
  - diagnostic publication
- How should package-level invalidation be represented when one changed file can stale other files' semantic results?
- How should hover degrade when the current file is mid-edit and semantically incomplete?

## Current discussion summary

- Current hover reruns lowering and naming unconditionally on every request.
- That mixes two separate costs:
  - target lookup cost
  - semantic recomputation cost
- `node_at_position` is a good syntax entry point for hover target discovery.
- It should not become the primary semantic identity.
- `ExpressionId` is currently stable only within one lowered module snapshot.
- Hover freshness and diagnostics publication do not need to use the same policy.
- rust-analyzer uses current edited buffer state for hover, but its visible diagnostics often lag because many come from external checking.
- LSP push diagnostics replace the previous diagnostics list for a document from that server.
- Therefore mixed-freshness diagnostics must be merged on the server if we want them shown together.
- rust-analyzer appears to keep separate internal diagnostic buckets and republish only files whose effective diagnostics changed.

## Working direction

- Use current-buffer state for hover.
- Avoid unconditional recomputation on hover.
- Publish cheap front-end diagnostics on `did_change`.
- Keep broader semantic diagnostics save-gated, or at least separately policy-controlled.
- Treat "computed" and "published" as different states.

## Hover target direction

- Use `node_at_position` to find the current syntax node.
- Walk ancestors until a semantic hover target is found.
- Add lowering provenance from syntax to semantic hover targets.
- Do not force the hover target model through only `ExpressionId`.

Possible target shape:

```rust
pub enum HoverTarget {
    Expression(ExpressionId),
    Definition(DefinitionId),
    Binding(BindingId),
    Parameter(BindingId),
}
```

## Diagnostics state direction

The early bucket is better described as front-end or lowering diagnostics than parser-only syntax diagnostics.

Smallest honest document/package-aware shape discussed so far:

```rust
pub struct DocumentState {
    pub version: DocumentVersion,
    pub front_end: PhaseState,
    pub semantic: SemanticState,
    pub last_full_publish: PublishedDiagnostics,
}

pub struct PhaseState {
    pub computed_at: Option<DocumentVersion>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct SemanticState {
    pub document_version: Option<DocumentVersion>,
    pub package_generation: u64,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct PublishedDiagnostics {
    pub diagnostics: Vec<Diagnostic>,
    pub front_end_version: Option<DocumentVersion>,
    pub semantic_document_version: Option<DocumentVersion>,
    pub semantic_package_generation: u64,
}
```

## Behavioral sketch

### On change

- increment document version
- rerun front-end/lowering for the changed document
- publish front-end diagnostics
- do not publish semantic diagnostics

### On hover

- ensure the needed semantic state is current for the document version and package generation
- do not publish semantic diagnostics as a side effect

### On save

- ensure front-end and semantic state are current
- merge the buckets according to the chosen editor policy
- publish only if the effective document diagnostics changed

## External comparison

### rust-analyzer

- hover uses current edited buffer state
- diagnostics are internally bucketed
- publication appears file-incremental rather than workspace-wide
- many visible diagnostics still come from external `cargo check`

### Gleam

- docs say unsaved edited versions are used when compiling in the language server
- no grounded source conclusion yet on hover implementation details

### Astral `ty`

- docs describe diagnostics updated while typing
- this suggests a stronger incremental foundation than `analysis` currently has

### Pyright / Pylance family

- diagnostics scope and timing are explicit policy knobs
- this is the closest precedent for "fresh language service, more conservative diagnostics publication"

## Tasks

- [pending] Decide the user-facing diagnostics policy during editing.
- [pending] Decide the minimum document/package state needed to support that policy honestly.
- [pending] Decide the hover target/provenance shape.
- [pending] Decide whether save-gated semantic diagnostics are the intended long-term product behavior or a migration policy.

