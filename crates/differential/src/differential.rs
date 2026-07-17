//! The cross-stack differential: the legacy analysis pipeline is the frozen
//! oracle for semantic diagnostics while the greenfield stack is built. The
//! comparison itself lives in this crate's tests; the parity scope is the
//! semantic diagnostic classes (typing, unresolved, unused) — the syntax
//! class is deliberately excluded, because the new parser's errors must be
//! better than the oracle's, not identical.
