# Typing Crate Discussion

This file is scratch space for active design discussion.

Use it for short summaries or temporary notes during a design conversation.

## Naming suite restructure

Resolved:

- the naming fixtures are split under:
  - `tests/naming/README.md`
  - `tests/naming/local/`
  - `tests/naming/global/`
- `README.md` is now the authoritative naming-suite contract
- `global` is the primary contract and mirrors the core lexical sections from `local`
- type-name coverage lives in `global`, not in a separate local type suite
- script-file naming behavior is now recorded in `SEMANTICS.md`

## Open decisions
