// The MIR battery (REARCH.md §12 R0.5). Two obligations are provable here with
// no other layer present:
//   (a) the Side-II encodability predicates of `isa.rs` against the A64 spec,
//       case by case — a wrong predicate is a Law-2 Side-II defect that would
//       otherwise surface as an assembler error deep inside a suite run;
//   (b) ⟦mir⟧ itself, on hand-built functions whose value is fixed by the ARM
//       manual's definition of the instruction, not by what zcc happens to emit.
use super::isa::*;
use super::*;

#[test]
fn add_immediate_range() {
    // DDI 0487 C6: add/sub take imm12, optionally shifted left by 12.
    assert_eq!(add_imm(0), Some((0, 0)));
    assert_eq!(add_imm(4095), Some((4095, 0)));
    assert_eq!(add_imm(4096), Some((1, 12)));
    assert_eq!(add_imm(0xfff_000), Some((0xfff, 12)));
    assert_eq!(add_imm(4097), None); // needs both fields
    assert_eq!(add_imm(-1), None); // the negative form is `sub`
    assert_eq!(add_imm(0x1_000_000), None);
}

#[test]
fn logical_immediate_encoding() {
    // A bitmask immediate is a rotated run of ones replicated across the register.
    assert!(logical_imm(1, true));
    assert!(logical_imm(0xff, true));
    assert!(logical_imm(0xf0, true));
    assert!(logical_imm(0xffff_ffff_ffff_fff8, true)); // ~7: a rotated run
    assert!(logical_imm(0x5555_5555_5555_5555, true)); // element size 2
    assert!(logical_imm(0x0f0f_0f0f_0f0f_0f0f, true)); // element size 8
    assert!(logical_imm(0xffff_ffff, true)); // element size 32
    // 0 and all-ones are NOT encodable — the encoding cannot represent them.
    assert!(!logical_imm(0, true));
    assert!(!logical_imm(u64::MAX, true));
    assert!(!logical_imm(0xffff_ffff, false)); // all-ones at 32 bits
    // two separate runs of ones
    assert!(!logical_imm(0b1011, true));
    assert!(!logical_imm(0x1234_5678, true));
}

#[test]
fn constant_materialization_chain() {
    use MovKind::*;
    assert_eq!(mov_chain(0, true), vec![(Z, 0, 0)]);
    assert_eq!(mov_chain(1, true), vec![(Z, 1, 0)]);
    assert_eq!(mov_chain(0xffff, true), vec![(Z, 0xffff, 0)]);
    assert_eq!(mov_chain(0x1_0000, true), vec![(Z, 1, 16)]);
    // `movn #imm` writes !(imm << shift): -1 is one `movn #0`, not four `movk`s
    assert_eq!(mov_chain(-1, true), vec![(N, 0, 0)]);
    assert_eq!(mov_chain(-65536, true), vec![(N, 0xffff, 0)]);
    assert_eq!(mov_chain(-2, true), vec![(N, 1, 0)]);
    let c = mov_chain(0x1234_5678_9abc_def0u64 as i64, true);
    assert_eq!(c.len(), 4);
    assert_eq!(c[0].0, Z);
    assert!(c[1..].iter().all(|e| e.0 == K));
    // and the chain reconstructs the value
    let mut v: u64 = 0;
    for (k, imm, sh) in &c {
        match k {
            Z => v = (*imm as u64) << sh,
            N => v = !((*imm as u64) << sh),
            K => v = (v & !(0xffffu64 << sh)) | ((*imm as u64) << sh),
        }
    }
    assert_eq!(v, 0x1234_5678_9abc_def0);
}

#[test]
fn memory_offset_forms() {
    // unsigned scaled: imm12 * size; signed: -256..255 unscaled
    assert!(mem_off_ok(0, 8));
    assert!(mem_off_ok(8, 8));
    assert!(mem_off_ok(4095 * 8, 8));
    assert!(!mem_off_ok(4096 * 8, 8));
    assert!(mem_off_ok(4, 8)); // unscaled signed-9 covers it
    assert!(mem_off_ok(-256, 8));
    assert!(!mem_off_ok(-257, 8));
    assert!(pair_off_ok(0, 8));
    assert!(pair_off_ok(-512, 8));
    assert!(!pair_off_ok(-520, 8));
    assert!(!pair_off_ok(4, 8));
}

#[test]
fn fp_immediate_form() {
    assert!(fp_imm8(1.0f64.to_bits(), Width::D));
    assert!(fp_imm8(0.5f64.to_bits(), Width::D));
    assert!(fp_imm8((-2.0f64).to_bits(), Width::D));
    assert!(fp_imm8(31.0f64.to_bits(), Width::D)); // 2^4 * (1+15/16)
    assert!(!fp_imm8(0.0f64.to_bits(), Width::D)); // zero needs `fmov d, xzr`
    assert!(!fp_imm8(3.14159f64.to_bits(), Width::D));
    assert!(!fp_imm8(f64::NAN.to_bits(), Width::D));
}

#[test]
fn register_files_are_the_full_spec_table() {
    // Article E's question: the spec's number, or a convenience truncation?
    // AAPCS64 §6.1.1 leaves x0–x15 and x19–x28 usable (x16/x17 IP0/IP1 reserved
    // for parallel-copy cycles, x18 platform, x29 FP, x30 LR, x31 SP).
    assert_eq!(k(Class::Gpr), 26); // 16 caller-saved + 10 callee-saved
    assert_eq!(GPR_ORDER.len(), 26);
    assert!(!GPR_ORDER.contains(&16) && !GPR_ORDER.contains(&17));
    assert!(!GPR_ORDER.contains(&18));
    assert!(!GPR_ORDER.contains(&29) && !GPR_ORDER.contains(&30));
    // caller-saved come first, so a value not live across a call never forces a
    // prologue save
    assert!(GPR_ORDER[..16].iter().all(|&n| !is_callee_saved(PReg::gpr(n))));
    assert!(GPR_ORDER[16..].iter().all(|&n| is_callee_saved(PReg::gpr(n))));
    // v31 is the FP scratch; everything else is allocatable
    assert_eq!(FPR_ORDER.len(), 31);
    assert!(!FPR_ORDER.contains(&31));
    let cs = caller_saved();
    assert!(cs.has(PReg::gpr(0)) && cs.has(PReg::gpr(15)));
    assert!(!cs.has(PReg::gpr(19)));
    assert!(cs.has(PReg::fpr(0)) && !cs.has(PReg::fpr(8)));
}

// ── ⟦mir⟧ on hand-built functions ──────────────────────────────────────────
fn mfunc(name: &str) -> MFunc {
    MFunc {
        name: name.into(),
        blocks: Vec::new(),
        vregs: Vec::new(),
        slots: Vec::new(),
        entry: 0,
        is_static: false,
        is_weak: false,
        order: Vec::new(),
        laid_out: false,
        frame_size: 0,
        saved: RegSet::default(),
        dyn_stack: false,
        has_vla: false,
        outgoing: 0,
        fp_slot: 0,
        physical: false,
    }
}

/// Run a one-block function that computes into x0 and returns.
fn run1(f: MFunc, args: &[u64]) -> u64 {
    let m = MModule { funcs: vec![f] };
    for f in &m.funcs {
        verify::verify(f).unwrap_or_else(|e| panic!("{}", e));
    }
    let ast = crate::ast::Ast {
        nodes: vec![],
        types: vec![],
        tt: crate::ast::TyTab::new(),
        funcs: vec![],
        globals: vec![],
        strs: vec![],
        raw_asm: vec![],
        aliases: vec![],
        pic: false,
        weak_decls: vec![],
    };
    let mut mach = interp::new_machine(&m, &ast);
    mach.call("f", args, &[]).expect("⟦mir⟧ trapped")
}

#[test]
fn alu_and_flags_semantics() {
    // f(a, b) = a + b, then compare and set: returns (a+b) with NZCV consulted
    let mut f = mfunc("f");
    let b0 = f.new_block();
    let s = f.new_vreg(Width::W64);
    let fl = f.new_flags();
    let r = f.new_vreg(Width::W64);
    f.blocks[b0 as usize].insts = vec![
        MInst::Alu {
            op: AluOp::Add,
            w: Width::W64,
            dst: s,
            a: Reg::P(PReg::gpr(0)),
            b: Rhs::Reg(Reg::P(PReg::gpr(1))),
            flags: None,
        },
        // cset x0, lt  after  cmp s, #10
        MInst::Cmp {
            kind: CmpKind::Cmp,
            w: Width::W64,
            a: s,
            b: Rhs::Imm(10),
            flags: fl,
        },
        MInst::CSet {
            w: Width::W64,
            dst: r,
            cc: CC::Lt,
            flags: fl,
        },
        MInst::Copy {
            w: Width::W64,
            dst: Reg::P(PReg::gpr(0)),
            src: r,
        },
    ];
    f.blocks[b0 as usize].term = MTerm::Ret;
    assert_eq!(run1(f.clone(), &[3, 4]), 1); // 7 < 10
    assert_eq!(run1(f, &[30, 4]), 0); // 34 >= 10
}

#[test]
fn w_form_results_are_zero_extended() {
    // The machine truth of A64: a 32-bit result clears the upper half.
    let mut f = mfunc("f");
    let b0 = f.new_block();
    let v = f.new_vreg(Width::W32);
    f.blocks[b0 as usize].insts = vec![
        MInst::Alu {
            op: AluOp::Sub,
            w: Width::W32,
            dst: v,
            a: Reg::P(PReg::gpr(0)),
            b: Rhs::Imm(1),
            flags: None,
        },
        MInst::Copy {
            w: Width::W64,
            dst: Reg::P(PReg::gpr(0)),
            src: v,
        },
    ];
    f.blocks[b0 as usize].term = MTerm::Ret;
    assert_eq!(run1(f, &[0]), 0xffff_ffff);
}

#[test]
fn block_parameters_carry_values_across_edges() {
    // f(n) = n == 0 ? 100 : 200, through a join block with one parameter.
    let mut f = mfunc("f");
    let (b0, bt, be, bj) = (f.new_block(), f.new_block(), f.new_block(), f.new_block());
    let p = f.new_vreg(Width::W64);
    let (a, b) = (f.new_vreg(Width::W64), f.new_vreg(Width::W64));
    f.blocks[bj as usize].params = vec![p];
    f.blocks[b0 as usize].term = MTerm::Cbz {
        w: Width::W64,
        reg: Reg::P(PReg::gpr(0)),
        zero: true,
        t: MTarget {
            block: bt,
            args: vec![],
        },
        f: MTarget {
            block: be,
            args: vec![],
        },
    };
    f.blocks[bt as usize].insts = vec![MInst::MovImm {
        w: Width::W64,
        dst: a,
        imm: 100,
    }];
    f.blocks[bt as usize].term = MTerm::B(MTarget {
        block: bj,
        args: vec![a],
    });
    f.blocks[be as usize].insts = vec![MInst::MovImm {
        w: Width::W64,
        dst: b,
        imm: 200,
    }];
    f.blocks[be as usize].term = MTerm::B(MTarget {
        block: bj,
        args: vec![b],
    });
    f.blocks[bj as usize].insts = vec![MInst::Copy {
        w: Width::W64,
        dst: Reg::P(PReg::gpr(0)),
        src: p,
    }];
    f.blocks[bj as usize].term = MTerm::Ret;
    assert_eq!(run1(f.clone(), &[0]), 100);
    assert_eq!(run1(f, &[5]), 200);
}

#[test]
fn verifier_rejects_broken_ssa() {
    let mut f = mfunc("f");
    let b0 = f.new_block();
    let v = f.new_vreg(Width::W64);
    f.blocks[b0 as usize].insts = vec![
        MInst::Copy {
            w: Width::W64,
            dst: v,
            src: Reg::P(PReg::gpr(0)),
        },
        MInst::Copy {
            w: Width::W64,
            dst: v,
            src: Reg::P(PReg::gpr(1)),
        },
    ];
    f.blocks[b0 as usize].term = MTerm::Ret;
    assert!(verify::verify(&f).is_err(), "double definition accepted");

    // a GPR where the encoding needs a v-register
    let mut g = mfunc("g");
    let b = g.new_block();
    let x = g.new_vreg(Width::W64);
    let y = g.new_vreg(Width::D);
    g.blocks[b as usize].insts = vec![MInst::FpAlu {
        op: FpOp::Fadd,
        w: Width::D,
        dst: y,
        a: x,
        b: y,
    }];
    g.blocks[b as usize].term = MTerm::Ret;
    assert!(verify::verify(&g).is_err(), "class violation accepted");
}
