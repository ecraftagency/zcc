// Frame-offset legalization (REARCH.md §8, post-frame).
//
// `pass/frame.rs` is the first place a stack object has a NUMBER, and A64's
// addressing modes bound that number: the unsigned form of a load/store reaches
// 4095 × the access size, the signed form ±256 (DDI 0487 C3.2). A frame larger
// than that is not an error and not a special case — it is an operand that must
// be legalized, and the legalization is an ordinary MIR rewrite: compute the
// address into the reserved second scratch (IP1, x17 — reserved exactly so a
// spill address can have a register, REARCH §5.1) and address through it.
//
// Doing this HERE rather than in `emit` is what keeps Article B's rule intact:
// the emitter makes no decisions and re-parses nothing. It is also what keeps
// the cost square exact — every instruction the legalization needs is a real
// `MInst` in the final function, so `cost(f) = |MIR_final(f)|` still holds.
//
// Its commuting square is the identity on ⟦·⟧: `Slot{s, off}` and
// `BaseImm{IP1, 0}` after `IP1 = &slot + off` denote the same address, and IP1
// is reserved, so no live value is destroyed.
use crate::mir::*;

pub fn run(f: &mut MFunc) {
    let offs: Vec<i32> = f.slots.iter().map(|s| s.off).collect();
    let scratch = Reg::P(isa::SCRATCH_GPR2);
    for b in f.blocks.iter_mut() {
        let insts = std::mem::take(&mut b.insts);
        let mut out = Vec::with_capacity(insts.len());
        for mut i in insts {
            match &mut i {
                MInst::Load { op, mem, .. } | MInst::Store { op, mem, .. } => {
                    if let Some(pre) = fixup(&offs, mem, op.bytes(), scratch) {
                        out.push(pre);
                    }
                }
                // A spill slot is addressed the same way; rewriting the pseudo
                // into the load/store it already was keeps ONE code path.
                MInst::Spill { slot, src, w } => {
                    let (slot, src, w) = (*slot, *src, *w);
                    if !isa::mem_off_ok(offs[slot as usize], w.bytes()) {
                        out.push(MInst::SlotAddr {
                            dst: scratch,
                            slot,
                            off: 0,
                        });
                        i = MInst::Store {
                            op: mem_op(w),
                            src,
                            mem: AddrMode::BaseImm {
                                base: scratch,
                                off: 0,
                            },
                            vol: false,
                        };
                    }
                }
                MInst::Reload { slot, dst, w } => {
                    let (slot, dst, w) = (*slot, *dst, *w);
                    if !isa::mem_off_ok(offs[slot as usize], w.bytes()) {
                        out.push(MInst::SlotAddr {
                            dst: scratch,
                            slot,
                            off: 0,
                        });
                        i = MInst::Load {
                            op: mem_op(w),
                            dst,
                            mem: AddrMode::BaseImm {
                                base: scratch,
                                off: 0,
                            },
                            vol: false,
                        };
                    }
                }
                _ => {}
            }
            out.push(i);
        }
        b.insts = out;
    }
}

/// The access form a spilled value of this width uses.
fn mem_op(w: Width) -> MemOp {
    match w {
        Width::W32 => MemOp::W,
        Width::W64 => MemOp::X,
        Width::S => MemOp::S,
        Width::D => MemOp::D,
        Width::Q => MemOp::Q,
    }
}

/// If `mem` names a frame object whose resolved offset does not fit its access
/// form, redirect it through the scratch and return the address computation.
fn fixup(offs: &[i32], mem: &mut AddrMode, size: u32, scratch: Reg) -> Option<MInst> {
    let pre = match *mem {
        AddrMode::Slot { slot, off } => {
            let o = offs[slot as usize] + off;
            if isa::mem_off_ok(o, size) {
                return None;
            }
            MInst::SlotAddr {
                dst: scratch,
                slot,
                off,
            }
        }
        AddrMode::SpArg { off } => {
            if isa::mem_off_ok(off as i32, size) {
                return None;
            }
            MInst::SpAddr { dst: scratch, off }
        }
        _ => return None,
    };
    *mem = AddrMode::BaseImm {
        base: scratch,
        off: 0,
    };
    Some(pre)
}
