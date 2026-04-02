# Discussion

This file is scratch space for active design discussion.

Use it for short summaries or temporary notes during a design conversation.

## IDE fixture runner

Current open direction:

- Keep the shared low-level `#++++ path` support in the `fixtures` crate.
- If IDE coverage expands beyond hover, prefer one combined IDE fixture runner in `analysis`.

Reasoning:

- `hover`, `rename`, `goto_definition`, and `assert_content` are explicit IDE requests over the
  current workspace state.
- That fits better with one request layer than with many suite-specific sidecar-file conventions.

Likely syntax split:

- `#----` for workspace mutations
- `#!!!!` for IDE requests such as:
  - `hover`
  - `rename`
  - `goto_definition`
  - `assert_content`
