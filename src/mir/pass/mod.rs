// MIR passes (REARCH.md §8). Pre-allocation passes run on SSA; post-allocation
// passes run on physical MIR. gcc -O1 has no instruction scheduler and neither
// does this list.
pub mod frame;
pub mod legalize;
#[cfg(test)]
mod tests;
pub mod layout;
