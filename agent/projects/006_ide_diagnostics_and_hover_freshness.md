[planning] IDE Diagnostics And Hover Freshness

Concept phase only. Not ready to be implemented.

* moving towards incremental analysis

* lowering (including syntax checks should always run after did_change)
* naming and type check phases shoudn't always run.
* (challenge this) as far as I understand lsp protocol only allows to publish diagnostics for entire file. ideally it would allow to publish diagnostics for single phase without overriding existing diagnostics. this would be interesting as we only want to show typing and naming errors on save but not delete them while you type. so one idea is to update a document version every time it was changed. then instead of tagging the diagnotcs by phase we have:

Diagnostics:
  lint: (u64, Vec<Diagnostics>)
  lower: (u64, Vec<Diagnostics>)
  naming: (u64, Vec<Diagnostics>) (maybe split by global and local in some way, we need to discuss)
  typecheck: (u64, Vec<Diagnostics>) (same here)

then we can run only some phases. when we publish diagnostics for a document we can consolidate them into one vec (they potentially have different origins)

internally we should also not re-run a phase if we already ran it (we need to compare current document version against last diagnostic version). 

certain actions like hovering or rename, require to also have next phases run so we have to run them lazily, when these ide actions are requested.


there are already some incremental mechanism in place. but we should consolidate it into one mechanism instead of multiple competing ones.

which documents have to be re-run should not be part of the api for now. (analysisstate should keep track of it and re-run for required documents)

so api becomes:

analysis::lint(state)
analysis::lower(state)
analysis::resolve_locals(state)
analysis::resovle_globals(state)
analysis::typecheck(state)
