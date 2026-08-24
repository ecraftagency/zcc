// Side-II: the A64 / AAPCS64 ultimate fact, transcribed (THEORY II-3, II-5;
// AAPCS64 IHI 0055 §6.1.1; ARM DDI 0487 C4/C6). Nothing in this file is a
// choice — every table and predicate is a spec line, and every deviation from
// one would be a Law-2 Side-II defect.
//
// Article E's mandatory question for each constant here is "the spec's number,
// or my convenience's number?". The allocatable sets below are therefore the
// FULL register files minus only registers the ABI itself reserves — never a
// truncated budget (the `GP_BUDGET.k=10` mistake of rc3).
use super::{Class, PReg, RegSet, Width};

// ── the register files ─────────────────────────────────────────────────────
// AAPCS64 §6.1.1: x0–x7 arguments/results, x8 indirect result, x9–x15 temporary,
// x16/x17 IP0/IP1 (intra-procedure-call scratch, clobberable by a linker veneer
// at ANY call — so zcc reserves them for its own parallel-copy cycle breaking),
// x18 platform register (reserved), x19–x28 callee-saved, x29 FP, x30 LR.
pub const FP: PReg = PReg::gpr(29);
pub const LR: PReg = PReg::gpr(30);
/// the stack pointer: encoded as register 31 in the base+offset forms
pub const SP: PReg = PReg::gpr(31);
/// zero register: the same encoding as SP, distinguished by the instruction form
pub const ZR: PReg = PReg::gpr(31);
/// GPR scratch reserved for cycle-breaking in a parallel copy (IP0)
pub const SCRATCH_GPR: PReg = PReg::gpr(16);
/// second scratch, used when a spill address needs its own register (IP1)
pub const SCRATCH_GPR2: PReg = PReg::gpr(17);
/// FPR scratch, same role as IP0 for the v-file
pub const SCRATCH_FPR: PReg = PReg::fpr(31);

/// Allocation order for the GPR class: caller-saved first, so a value that does
/// not live across a call never forces a prologue save; callee-saved last, so a
/// value that does live across one lands there without any "crossing" rule.
pub const GPR_ORDER: [u8; 26] = [
    // caller-saved (x0–x7 are also the argument registers)
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, //
    // callee-saved
    19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
];

/// v0–v7 (arguments), v16–v30 (temporary), then the callee-saved v8–v15.
/// AAPCS64 §6.1.2: only the low 64 bits of v8–v15 are preserved, which is exactly
/// what zcc stores (no NEON values yet, REARCH §16 row 13).
pub const FPR_ORDER: [u8; 31] = [
    0, 1, 2, 3, 4, 5, 6, 7, //
    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, //
    8, 9, 10, 11, 12, 13, 14, 15,
];

pub fn alloc_order(c: Class) -> &'static [u8] {
    match c {
        Class::Gpr => &GPR_ORDER,
        Class::Fpr => &FPR_ORDER,
        Class::Flags => &[0],
    }
}

/// The allocatable registers of a class, as a bit mask. `k(class)` is its
/// population count; a clobber set intersected with it is the number of colours
/// a call actually takes away.
pub fn alloc_mask(c: Class) -> u32 {
    alloc_order(c).iter().fold(0u32, |m, &n| m | 1 << n)
}

/// `k` for the coloring theorem: the number of assignable colors in a class.
pub fn k(c: Class) -> usize {
    alloc_order(c).len()
}

pub fn is_callee_saved(p: PReg) -> bool {
    match p.class {
        Class::Gpr => (19..=28).contains(&p.num),
        Class::Fpr => (8..=15).contains(&p.num),
        Class::Flags => false,
    }
}

/// AAPCS64 §6.1.1: the registers a call may destroy. The allocator needs no
/// other notion of a call: this set becomes fixed definitions at the call site.
pub fn caller_saved() -> RegSet {
    let mut s = RegSet::default();
    for n in 0..=18u8 {
        s.add(PReg::gpr(n));
    }
    for n in 0..=7u8 {
        s.add(PReg::fpr(n));
    }
    for n in 16..=31u8 {
        s.add(PReg::fpr(n));
    }
    s
}

// ── register names (the emitter's only naming authority) ───────────────────
pub fn gpr_name(n: u8, is64: bool) -> String {
    match (n, is64) {
        (31, true) => "xzr".into(),
        (31, false) => "wzr".into(),
        (29, true) => "x29".into(),
        (30, true) => "x30".into(),
        (n, true) => format!("x{}", n),
        (n, false) => format!("w{}", n),
    }
}

pub fn sp_name() -> &'static str {
    "sp"
}

pub fn fpr_name(n: u8, w: Width) -> String {
    match w {
        Width::S => format!("s{}", n),
        Width::Q => format!("q{}", n),
        _ => format!("d{}", n),
    }
}

pub fn reg_name(p: PReg, w: Width) -> String {
    match p.class {
        Class::Gpr => gpr_name(p.num, w.is64()),
        Class::Fpr => fpr_name(p.num, w),
        Class::Flags => "nzcv".into(),
    }
}

// ── immediate encodability (DDI 0487 C4.1) ─────────────────────────────────
/// `add/sub/cmp` immediate: a 12-bit unsigned value, optionally shifted left 12.
/// Returns the (value, shift) pair when encodable.
pub fn add_imm(v: i64) -> Option<(u32, u8)> {
    if (0..=0xfff).contains(&v) {
        Some((v as u32, 0))
    } else if v & 0xfff == 0 && (0..=0xfff_000).contains(&v) {
        Some((v as u32 >> 12, 12))
    } else {
        None
    }
}

/// The bitmask ("logical") immediate of `and/orr/eor/tst`: a value that is the
/// replication, across the register, of a rotated run of ones in an element of
/// size 2, 4, 8, 16, 32 or 64 bits. DDI 0487 C4.1.4 / J1 `DecodeBitMasks`.
pub fn logical_imm(v: u64, is64: bool) -> bool {
    let width = if is64 { 64 } else { 32 };
    let v = if is64 { v } else { v & 0xffff_ffff };
    if v == 0 || (width == 64 && v == u64::MAX) || (width == 32 && v == 0xffff_ffff) {
        return false;
    }
    // the smallest element size at which `v` is a replication (size == width
    // always qualifies, so the search terminates)
    let mut size = 2;
    loop {
        let mask = if size >= 64 { u64::MAX } else { (1u64 << size) - 1 };
        let lo = v & mask;
        let mut ok = true;
        let mut i = size;
        while i < width {
            if (v >> i) & mask != lo {
                ok = false;
                break;
            }
            i += size;
        }
        if ok {
            break;
        }
        size *= 2;
    }
    let mask = if size >= 64 {
        u64::MAX
    } else {
        (1u64 << size) - 1
    };
    let e = v & mask;
    if e == 0 || e == mask {
        return false;
    }
    // A rotated run of ones: rotate the element right until the run starts at
    // bit 0 without wrapping (low bit set, high bit clear); it is then encodable
    // exactly when the result is `(1 << ones) - 1`.
    let rotated = {
        let mut x = e;
        for _ in 0..size {
            if x & 1 == 1 && (x >> (size - 1)) & 1 == 0 {
                break;
            }
            x = (x >> 1) | ((x & 1) << (size - 1));
        }
        x
    };
    let ones = rotated.count_ones() as u64;
    ones < size && rotated == ((1u64 << ones) - 1)
}

/// The `movz/movn/movk` chain that materializes `v`: the first entry is a
/// `movz` or `movn`, the rest are `movk`. Never longer than 4 instructions.
pub fn mov_chain(v: i64, is64: bool) -> Vec<(super::MovKind, u16, u8)> {
    use super::MovKind::{K, N, Z};
    let v = if is64 { v as u64 } else { v as u32 as u64 };
    let halves = if is64 { 4 } else { 2 };
    let hw = |i: u8| ((v >> (16 * i as u32)) & 0xffff) as u16;
    let inv = !v & if is64 { u64::MAX } else { 0xffff_ffff };
    let ihw = |i: u8| ((inv >> (16 * i as u32)) & 0xffff) as u16;
    // `movz` clears every other halfword, so only the nonzero ones need writing;
    // `movn` sets every other halfword to all-ones, so only the halfwords that
    // are NOT all-ones need writing. Build both and keep the shorter chain.
    let nonzero: Vec<u8> = (0..halves).filter(|&i| hw(i) != 0).collect();
    let inz: Vec<u8> = (0..halves).filter(|&i| ihw(i) != 0).collect();
    if nonzero.is_empty() {
        return vec![(Z, 0, 0)];
    }
    if inz.is_empty() {
        return vec![(N, 0, 0)];
    }
    let zc: Vec<_> = nonzero
        .iter()
        .enumerate()
        .map(|(j, &i)| (if j == 0 { Z } else { K }, hw(i), i * 16))
        .collect();
    let nc: Vec<_> = inz
        .iter()
        .enumerate()
        .map(|(j, &i)| {
            if j == 0 {
                (N, ihw(i), i * 16)
            } else {
                (K, hw(i), i * 16)
            }
        })
        .collect();
    if nc.len() < zc.len() { nc } else { zc }
}

/// Load/store offset legality: the unsigned form scales by the access size and
/// spans 12 bits; the signed form is a 9-bit byte offset. DDI 0487 C3.2.
pub fn mem_off_ok(off: i32, size: u32) -> bool {
    (off >= 0 && off as u32 % size == 0 && (off as u32 / size) <= 4095) || (-256..=255).contains(&off)
}

/// `ldp/stp`: a 7-bit offset scaled by the access size.
pub fn pair_off_ok(off: i32, size: u32) -> bool {
    off % size as i32 == 0 && (-64..=63).contains(&(off / size as i32))
}

/// `fmov` 8-bit immediate (DDI 0487 C7 `VFPExpandImm`): sign · 2^e · (1 + m/16)
/// with a 3-bit exponent and 4-bit mantissa. Anything else needs a literal load.
pub fn fp_imm8(bits: u64, w: Width) -> bool {
    let x = match w {
        Width::S => f32::from_bits(bits as u32) as f64,
        _ => f64::from_bits(bits),
    };
    if x == 0.0 || !x.is_finite() {
        return false;
    }
    let a = x.abs();
    let e = a.log2().floor() as i32;
    if !(-3..=4).contains(&e) {
        return false;
    }
    let m = a / 2f64.powi(e) - 1.0;
    let q = (m * 16.0).round();
    (0.0..=15.0).contains(&q) && 2f64.powi(e) * (1.0 + q / 16.0) == a
}
