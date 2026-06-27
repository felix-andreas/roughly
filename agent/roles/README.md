# Agent roles

This directory defines the **roles** used to drive Roughly's AI-led development as a small organization (a CEO/director orchestrating a CTO and an expert reviewer, each running their own subagents).

Each `*.md` file is one role definition: its mandate, what it does and explicitly does **not** do, how it operates, and its relationship to the other roles.

## Roles must be explicitly activated

**A role file existing here does NOT mean the role is in effect.** Every role is **dormant by default** and takes effect only when it has been **explicitly activated** for a given agent/session — e.g. the human assigns it via a direct instruction or a command (such as `/goal …`).

Rules:

- An agent must **not** assume a role merely because a definition exists in this directory.
- A role applies only to the agent it was explicitly activated for, and only for that session/engagement. Activation does not carry over implicitly to other agents or future sessions.
- One activation grants one role. An agent does not self-promote into another role (e.g. a CTO does not assume the CEO role) without its own explicit activation.
- When no role is activated, the default assistant behavior applies — these definitions are inert reference material, not standing instructions.

This keeps the org explicit and auditable: at any moment it is clear *who* is acting under *which* role and *by whose activation*.

## Current roles

| File | Role | Activated by |
|------|------|--------------|
| [`ceo.md`](./ceo.md) | CEO / Director — owns outcomes & direction; orchestrates, verifies, decides; writes no code | human (e.g. `/goal …`) |
| `cto.md` | CTO — owns all implementation via its own coder/architect/reviewer/researcher team *(definition TBD)* | CEO |
| `expert.md` | RA/LSP Expert — adversarial technical advisor & acceptance authority *(definition TBD)* | CEO |

Add a role by writing its `*.md` definition here and listing it above. The role still does nothing until explicitly activated.

> Note: these files are intentional project artifacts, not scratch. Do not delete this directory during cleanup.
