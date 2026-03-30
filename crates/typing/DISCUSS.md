# Typing Crate Discussion

This file is scratch space for active design discussion.

Use it for short summaries or temporary notes during a design conversation.

## Current topic

None currently recorded.

## Settled points

- Naming should split into file-local preparation and project-global resolution.
- The file-local naming pass should resolve local lexical facts eagerly.
- The project-global naming pass should assign distinct project-level ids for top-level declarations.
- Fixture runners now return `Result<Vec<FixtureOutput>, String>`.
- Each `FixtureOutput` is one snapshot with per-file outputs keyed by path.
- `Err(...)` is runner failure, not a rendered phase result.
- Expectations carry forward across generations by path.
- `#++++ any` means the immediately preceding file or operation is expected, but its contents are
  not asserted.
- `delete` removes the carried expectation for that path.
- `move` carries the expectation from the source path to the destination path.
- Extra actual outputs beyond expected paths are fixture failures.

## Open decisions

- None currently recorded.
