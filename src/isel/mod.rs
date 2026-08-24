// Instruction selection: HIR → MIR (REARCH.md §6).
//
// The layer is split along the Law-1 seam. `abi.rs` is Side II — the AAPCS64
// automaton over a call's C signature, pure transcribed spec. `imm.rs` is the
// legalization of constants against `mir::isa`'s encodability predicates, also
// Side II. `lower.rs` is Side I — the algorithm that turns an HIR tree into a
// machine sequence, and the place where every `⟦hir-tree⟧ = ⟦mir-seq⟧` theorem
// is realized.
pub mod abi;
pub mod imm;
pub mod lower;
#[cfg(test)]
mod tests;

pub use lower::lower;
