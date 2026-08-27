// MIR passes (REARCH.md §8). Pre-allocation passes run on SSA; post-allocation
// THEORY A6b — MIR; THEORY A7b — proving a machine pass
// passes run on physical MIR. gcc -O1 has no instruction scheduler; this list
// has one (`sched`, R5.4), default-off behind `ZCC_SCHED` until it is measured
// on a machine — Law 3c's claim is that a chain costs cycles a count cannot
// see, and the pass that acts on it owes a number, not an argument.
pub mod frame;
pub mod frame_fold;
pub mod legalize;
#[cfg(test)]
mod tests;
pub mod autoinc;
pub mod cmpelim;
pub mod const_share;
pub mod ext;
pub mod layout;
pub mod ldstp;
pub mod sched;
pub mod shrink_wrap;
pub mod slotmerge;
