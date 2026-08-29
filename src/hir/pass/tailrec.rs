// tailrec — a self-call in tail position becomes a jump back to the entry.
// THEORY A7b — optimization: this pass ships its commuting square
//
// WHY, and the measurement is unusually blunt. `e1_recursion` is the worst
// program in the 96-program suite at **3.00x** against gcc -O2, and it is the
// only kind of program where zcc emits FEWER static instructions than gcc by a
// factor of three (INSN 0.323) and still loses by a factor of three. gcc -O1 and
// -O2 both leave ZERO calls to `acker_lite` inside `acker_lite`; zcc leaves
// three, two of them in tail position. Every one of those pays a stack frame, a
// `bl`, a `ret`, and a call-clobbered register file it must save around.
//
// THE TRANSFORM. A `Call` to THIS function whose result is the value the block
// immediately returns is not a call: it is an assignment to the parameters
// followed by a jump to the top. The entry block gains parameters, the original
// argument values are handed to it once on the way in, and each tail call becomes
// `Jmp(entry, args)`.
//
// COMMUTING SQUARE. The callee is this function, so the code it would run is the
// code already here; nothing in the caller's frame is read after the call — the
// return value is returned unchanged and the block ends — so re-entering the top
// with the new arguments computes exactly what the call would have returned, and
// returns it from the same `Ret`. The recursion becomes iteration, which is the
// same sequence of states with one activation record instead of many.
//
// THE FENCE, and it is about the FRAME rather than about the call. Iteration
// REUSES the frame the recursion would have given a fresh copy of. That is
// invisible unless the address of something in the frame can be observed, so a
// function whose local addresses ESCAPE — passed to a call, or stored to memory —
// is refused, as are variadic functions (a `va_list` points into the frame) and
// functions with a VLA (whose frame is not a fixed shape to reuse).
use super::*;

/// THEORY A7b — the pass ships ON: `ZCC_NOTAILREC` turns it off.
pub fn wanted() -> bool {
    WANT.with(|c| c.get()).unwrap_or_else(|| {
        static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *W.get_or_init(|| std::env::var_os("ZCC_NOTAILREC").is_none())
    })
}

thread_local! {
    // THEORY A7b — instrument half. Not a value the compiler computes with: the
    // switch a battery flips to prove the rewrite happened. A thread-local for
    // the reason `spill.rs`'s seams are: the battery runs in parallel threads.
    static WANT: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub fn set_wanted(on: Option<bool>) {
    WANT.with(|c| c.set(on));
}

/// THEORY A7b — instrument half: tail calls turned into edges.
pub static FIRED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Does any address of this frame escape? An address that only ever names the
/// operand of a load or a store stays inside the function, and reusing the frame
/// is then unobservable. Anything else — handed to a call, stored somewhere,
/// returned — could be compared or dereferenced after the reuse.
fn frame_escapes(f: &Func) -> bool {
    let mut addrs: Vec<ValueId> = Vec::new();
    for b in &f.blocks {
        for inst in &b.insts {
            if let Inst::SlotAddr { dst, .. } | Inst::Alloca { dst, .. } = inst {
                addrs.push(*dst);
            }
        }
    }
    if addrs.is_empty() {
        return false;
    }
    let mut escaped = false;
    let mut watch = |o: Operand| {
        if let Operand::Val(v) = o {
            if addrs.contains(&v) {
                escaped = true;
            }
        }
    };
    for b in &f.blocks {
        for inst in &b.insts {
            match inst {
                // The one benign use: naming the place a load reads or a store
                // writes. The VALUE a store writes is not benign — that is the
                // address being put somewhere it outlives the frame.
                Inst::Load { .. } => {}
                Inst::Store { val, .. } => watch(*val),
                other => other.uses(&mut watch),
            }
        }
        b.term.uses(&mut watch);
    }
    escaped
}

/// THEORY A7b  SQUARE a_self_call_in_tail_position_is_a_jump — recursion into iteration
pub fn run(f: &mut Func) -> bool {
    if !wanted() || f.sig.variadic || f.has_vla || frame_escapes(f) {
        return false;
    }
    // A tail self-call: the LAST instruction of a block, its result the value the
    // block returns, and nothing between them.
    let nargs = f.sig.params.len();
    // ONLY the block index is recorded. The arguments are read back AFTER the
    // parameter rewrite below, because a tail call whose arguments ARE the
    // parameters — `Hanoi(n-1, spare, dest, source)`, a permutation of its own —
    // must pass the CURRENT values, not the ones the function was entered with.
    // Reading them early made the loop restart from the original arguments every
    // turn; `c-testsuite/00181` is what caught it and no unit test did.
    let mut sites: Vec<usize> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let last = match b.insts.last() {
            Some(i) => i,
            None => continue,
        };
        let (dst, callee, args, sret) = match last {
            Inst::Call { dst, callee, args, sret, .. } => (*dst, callee, args, *sret),
            _ => continue,
        };
        if sret.is_some() || args.len() != nargs {
            continue;
        }
        if !matches!(callee, Callee::Direct(n) if *n == f.name) {
            continue;
        }
        let tail = match (&b.term, dst) {
            (Term::Ret(Some(Operand::Val(r))), Some(d)) => *r == d,
            (Term::Ret(None), None) => true,
            _ => false,
        };
        if tail {
            sites.push(bi);
        }
    }
    if sites.is_empty() {
        return false;
    }

    // The entry becomes a LOOP HEADER: it takes the arguments as parameters, and
    // a new entry hands it the incoming ones once.
    let old = f.entry;
    let mut ps: Vec<ValueId> = Vec::new();
    for k in 0..nargs {
        let ty = f
            .values
            .iter()
            .position(|v| matches!(v.def, Def::FuncParam(x) if x as usize == k))
            .map(|v| f.values[v].ty)
            .unwrap_or(Ty::I64);
        let p = f.new_value(ty, Def::Param(old, k as u32));
        ps.push(p);
    }
    // Rewrite every read of the k-th incoming parameter to the k-th block
    // parameter. The incoming values survive as the argument the new entry passes.
    let mut argv: Vec<Option<ValueId>> = vec![None; nargs];
    for (i, v) in f.values.iter().enumerate() {
        if let Def::FuncParam(k) = v.def {
            if (k as usize) < nargs {
                argv[k as usize] = Some(i as ValueId);
            }
        }
    }
    let sub: Vec<(ValueId, ValueId)> = argv
        .iter()
        .enumerate()
        .filter_map(|(k, a)| a.map(|a| (a, ps[k])))
        .collect();
    for b in f.blocks.iter_mut() {
        let mut fix = |o: &mut Operand| {
            if let Operand::Val(v) = o {
                if let Some(&(_, np)) = sub.iter().find(|(a, _)| a == v) {
                    *o = Operand::Val(np);
                }
            }
        };
        for inst in b.insts.iter_mut() {
            inst.uses_mut(&mut fix);
        }
        let mut t = b.term.clone();
        t.uses_mut(&mut fix);
        for tg in t.targets_mut() {
            for a in tg.args.iter_mut() {
                fix(a);
            }
        }
        b.term = t;
    }
    f.blocks[old as usize].params.splice(0..0, ps.iter().copied());

    // Every existing edge into the old entry — there is none from inside a
    // well-formed function, but a `goto` to the top would make one — must now
    // carry the parameters.
    let entry_args: Vec<Operand> = argv
        .iter()
        .map(|a| a.map(Operand::Val).unwrap_or(Operand::Imm(0)))
        .collect();
    let neu = f.new_block();
    f.blocks[neu as usize].term = Term::Jmp(Target { block: old, args: entry_args });
    f.entry = neu;

    for bi in sites {
        let args = match f.blocks[bi].insts.pop() {
            Some(Inst::Call { args, .. }) => args,
            other => unreachable!("the tail site stopped being a call: {:?}", other),
        };
        f.blocks[bi].term = Term::Jmp(Target { block: old, args });
        FIRED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    refresh_defs(f);
    true
}
