//! The cross-stack BENCHMARK harness: `tests/test_stats.rs` runs the same
//! corpus through the frozen legacy stack and the shipping crates and
//! compares wall time and memory. The identity-parity program this crate
//! once hosted (typing/ide/format/corpus differential arms with adjudicated
//! divergence ledgers) is complete and retired by user decision: the new
//! stack no longer proves equivalence to the oracle — its fixtures are the
//! contract. This crate remains only until the legacy deletion sweep, and
//! every dependency edge it has points at the oracle so that sweep stays one
//! directory removal (after moving the new-stack-only perf witnesses out).
