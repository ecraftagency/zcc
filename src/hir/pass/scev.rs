// scev — scalar evolution for affine induction variables (REARCH §13f).
// THEORY A7b — optimization: this pass ships its commuting square
//
// An ANALYSIS, like `purity.rs`: it rewrites nothing and so carries no commuting
// square. What it owes instead is that every AddRec it returns is TRUE of the
// program, because three transforms are queued behind it and each turns a false
// evolution into a miscompile — pointer-IV (§13d cause #2), final-value, LFTR.
//
// THE FORM. A value's evolution in a loop is written `{base + off, +, step}`:
// on iteration n (counting the first as 0) it equals `base + off + step*n`,
// where `base` is a single LOOP-INVARIANT value (or nothing, for a pure
// constant) and `off`/`step` are integers. This is the affine fragment of
// Bachmann–Wegman–Zadeck chains of recurrences, restricted on purpose: one
// symbolic term, constant step, no nesting. Every shape the three consumers
// need is in it — `i`, `p + i*4`, `n - i` — and everything outside it returns
// `None` rather than an approximation.
//
// WHAT IS DELIBERATELY NOT CLAIMED, because claiming it would be unsound here:
//
//   * WIDENING. `sext(i)` for an i32 induction variable is NOT reported as an
//     AddRec. It is affine only while `i` does not wrap, and SEMANTICS.md §7
//     defines signed overflow as WRAPPING rather than as ⊥ — so the usual "signed
//     overflow is undefined, therefore assume no wrap" licence is not available
//     in this compiler. A consumer that wants the 64-bit evolution of a 32-bit
//     counter must first prove the trip count keeps it in range, and
//     `trip_count` is what it proves that with.
//   * ARITHMETIC WIDTH. `off` and `step` are accumulated in `i64` with wrapping.
//     For a value of type `ty` the recurrence is true modulo `2^ty.bits()`, which
//     is exactly what the machine does; a consumer reading them as unbounded
//     integers is reading them wrongly.
//   * NON-CONSTANT STEPS. A step that is loop-invariant but not a literal is
//     refused. It is representable and it is not needed by any queued consumer,
//     so it stays out until one asks (Article A).
use super::*;
use std::collections::HashMap;

/// `base + off + step*n` on iteration n, with `base` a loop-invariant value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AddRec {
    pub base: Option<ValueId>,
    pub off: i64,
    pub step: i64,
    pub ty: Ty,
}

impl AddRec {
    pub fn is_invariant(&self) -> bool {
        self.step == 0
    }
    /// The value on iteration `n`, when there is no symbolic base.
    pub fn at(&self, n: i64) -> Option<i64> {
        match self.base {
            None => Some(self.off.wrapping_add(self.step.wrapping_mul(n))),
            Some(_) => None,
        }
    }
}

/// One loop's induction variables, plus enough of its shape to evaluate a value
/// against them.
pub struct LoopScev {
    pub header: BlockId,
    /// Where a consumer would PLACE code, when the loop has such a block at all.
    /// It often does not: a rotated loop is entered straight from its guard, and
    /// the guard has two successors. The analysis never needs it — invariance is
    /// decided by the loop body, not by a block — so it is recorded for the
    /// consumers that will and is not a preconditon here.
    pub preheader: Option<BlockId>,
    /// blocks of the loop, indexed by block id
    inloop: Vec<bool>,
    /// the BASIC induction variables: header parameters that advance by a
    /// constant on every latch edge
    pub ivs: HashMap<ValueId, AddRec>,
    /// how many times the body runs, when that is exactly knowable. Computed
    /// once in `analyze`, because `eval` READS it: whether a narrowing
    /// conversion is affine depends on the counter staying in range, and that is
    /// a question about the trip count.
    pub trips: Option<u64>,
    /// Basic induction variables the loop's own exit test proves cannot wrap —
    /// separately for the signed and the unsigned reading, since a widening asks
    /// one question or the other and never both.
    nowrap_signed: std::collections::HashSet<ValueId>,
    nowrap_unsigned: std::collections::HashSet<ValueId>,
    latches: Vec<BlockId>,
}

impl LoopScev {
    /// Analyse loop `li`.
    pub fn analyze(
        f: &Func,
        c: &dom::Cfg,
        dt: &dom::DomTree,
        lf: &dom::LoopForest,
        li: usize,
    ) -> Option<LoopScev> {
        let header = lf.loops[li].header;
        let mut inloop = vec![false; f.blocks.len()];
        for &b in &lf.loops[li].body {
            inloop[b as usize] = true;
        }
        // Loop-invariant is "defined OUTSIDE the loop", and on SSA that is the
        // whole of it: a definition used inside the loop dominates its uses, so
        // one that sits outside dominates the header and is therefore evaluated
        // once, before the first iteration. Phrasing it as "dominates the
        // preheader" — licm's rule, because licm must also have somewhere to
        // move code TO — would be wrong here, since a rotated loop is entered
        // straight from its guard and has no preheader at all.
        let pre = preheader_of(f, c, dt, header);
        let mut s = LoopScev {
            header,
            preheader: pre,
            inloop,
            ivs: HashMap::new(),
            trips: None,
            nowrap_signed: std::collections::HashSet::new(),
            nowrap_unsigned: std::collections::HashSet::new(),
            latches: lf.loops[li].latches.clone(),
        };
        s.find_basic_ivs(f, c);
        s.trips = s.compute_trips(f);
        s.find_nowrap(f);
        Some(s)
    }

    /// Which basic induction variables the loop's OWN EXIT TEST keeps inside
    /// their type — the fact that lets `p[i]` be strength-reduced in a loop whose
    /// bound is a parameter, where no trip count exists.
    ///
    /// THE ARGUMENT, and it needs no numbers. Take `for (i = s; i < n; i++)` with
    /// `i` and `n` both `int`. Inside the body the test has passed, so `i < n`;
    /// and `n` IS an `int`, so `n ≤ INT_MAX`; so `i < INT_MAX`. The counter
    /// cannot leave its type while the body runs, whatever `n` happens to be.
    /// Nothing about the VALUE of the bound is used — only its TYPE — which is
    /// why this works where `trip_count` cannot.
    ///
    /// THE STEP MUST BE ±1, and that restriction is the whole of the soundness.
    /// With `i += 2` and `n == INT_MAX` the counter runs …, INT_MAX-1, and the
    /// next increment overflows — the test then sees a NEGATIVE value, says
    /// "stay in", and the loop walks off with an address this analysis would have
    /// promised was affine. At ±1 the increment lands exactly on the bound and
    /// the loop leaves. (SEMANTICS §7 makes that overflow DEFINED, so it is a
    /// real execution to account for, not undefined behaviour to assume away.)
    ///
    /// Signed and unsigned are tracked apart because the reading has to match the
    /// widening: `sext` needs the SIGNED comparison to have bounded the counter,
    /// `zext` the unsigned one.
    fn find_nowrap(&mut self, f: &Func) {
        let (op, lhs, rhs, ty) = match self.exit_test(f) {
            Some(x) => x,
            None => return,
        };
        // The bound must be a value of the compared type, not another recurrence.
        match self.eval(f, rhs) {
            Some(b) if b.is_invariant() => {}
            _ => return,
        }
        let ivs: Vec<(ValueId, AddRec)> = self.ivs.iter().map(|(k, v)| (*k, *v)).collect();
        for (p, rec) in ivs {
            if rec.ty != ty || rec.step.abs() != 1 {
                continue;
            }
            // The tested value must BE this counter, give or take a constant —
            // otherwise the test bounds some other value and says nothing here.
            if !self.is_offset_of(f, lhs, p) {
                continue;
            }
            let up = rec.step > 0;
            match (op, up) {
                (CmpOp::Slt | CmpOp::Sle, true) | (CmpOp::Sgt | CmpOp::Sge, false) => {
                    self.nowrap_signed.insert(p);
                }
                (CmpOp::Ult | CmpOp::Ule, true) | (CmpOp::Ugt | CmpOp::Uge, false) => {
                    self.nowrap_unsigned.insert(p);
                }
                _ => {}
            }
        }
    }

    /// Is this value defined OUTSIDE the loop?
    ///
    /// Distinct from `AddRec::is_invariant`, which only says the recurrence has
    /// step 0 — true of plenty of values COMPUTED INSIDE the loop, such as the
    /// sum of two invariants. A caller that wants to move something out of the
    /// loop needs this one; asking the other put a `sext` of a header-defined
    /// value into the preheader and broke sqlite (§13l).
    /// ASKED, NOT TABULATED. A value is loop-invariant when its definition sits
    /// outside the loop, which is one lookup — where building the answer for
    /// every value in the function, once per loop, cost the whole value space per
    /// loop and was read for a handful of them.
    pub fn is_loop_invariant(&self, f: &Func, v: ValueId) -> bool {
        match f.values[v as usize].def {
            Def::FuncParam(_) => true,
            Def::Inst(b, _) | Def::Param(b, _) => !self.inloop[b as usize],
        }
    }

    /// Does the loop's own exit test prove this basic induction variable cannot
    /// leave its type, read as SIGNED? See `find_nowrap` for the argument.
    pub fn no_wrap_signed(&self, v: ValueId) -> bool {
        self.nowrap_signed.contains(&v)
    }

    /// `o` is `p` itself, or `p` plus a literal.
    fn is_offset_of(&self, f: &Func, o: Operand, p: ValueId) -> bool {
        let v = match o.val() {
            Some(v) => v,
            None => return false,
        };
        if v == p {
            return true;
        }
        matches!(
            self.def_inst(f, v),
            Some(Inst::Bin { op: BinOp::Add | BinOp::Sub, a: Operand::Val(x), b: Operand::Imm(_), .. })
                if *x == p
        )
    }

    /// The loop's single exit test, normalized to "stay in the loop while
    /// LHS <op> RHS", with the type the comparison is made at.
    fn exit_test(&self, f: &Func) -> Option<(CmpOp, Operand, Operand, Ty)> {
        let mut exiting = None;
        for b in 0..f.blocks.len() as BlockId {
            if !self.inloop[b as usize] {
                continue;
            }
            if f.blocks[b as usize]
                .term
                .succs()
                .iter()
                .any(|&s| !self.inloop[s as usize])
            {
                if exiting.is_some() {
                    return None;
                }
                exiting = Some(b);
            }
        }
        let ex = exiting?;
        let (cond, taken_in) = match &f.blocks[ex as usize].term {
            Term::Br(cond, t, e) => {
                match (self.inloop[t.block as usize], self.inloop[e.block as usize]) {
                    (true, false) => (*cond, true),
                    (false, true) => (*cond, false),
                    _ => return None,
                }
            }
            _ => return None,
        };
        let (op, a, b, ty) = match self.def_inst(f, cond.val()?)? {
            Inst::Cmp { op, a, b, ty, .. } => (*op, *a, *b, *ty),
            _ => return None,
        };
        let op = if taken_in { op } else { invert(op)? };
        Some((op, a, b, ty))
    }

    /// The value `x`, of a `bits`-wide type, stays inside that type for every
    /// iteration the loop runs — so widening it is the identity and its
    /// recurrence carries to the wider type unchanged.
    ///
    /// This is the ONE place the trip count is load-bearing for correctness
    /// rather than for a rewrite, and it is load-bearing because SEMANTICS.md §7
    /// defines signed overflow as WRAPPING. Every other compiler gets this for
    /// free from "signed overflow is undefined"; this one has to prove it.
    fn stays_in_range(&self, x: &AddRec, bits: u32, signed: bool) -> bool {
        let n = match self.trips {
            Some(n) => n as i128,
            None => return false,
        };
        if x.base.is_some() {
            return false;
        }
        let (lo, hi) = match signed {
            true => (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1),
            false => (0i128, (1i128 << bits) - 1),
        };
        // n+1 values are reachable: one per body execution, plus the value the
        // counter holds when the test that ends the loop reads it.
        let first = x.off as i128;
        let last = first + (x.step as i128) * n;
        let (a, b) = if last < first { (last, first) } else { (first, last) };
        a >= lo && b <= hi
    }

    /// A header parameter is a basic induction variable when the preheader gives
    /// it a loop-invariant start and EVERY latch gives it that same parameter
    /// plus one constant. "Every latch" is not pedantry: a loop with two back
    /// edges that advance by different amounts has no single step, and a rule
    /// that looked at one of them would invent one.
    fn find_basic_ivs(&mut self, f: &Func, c: &dom::Cfg) {
        let hp = f.blocks[self.header as usize].params.clone();
        for (k, &p) in hp.iter().enumerate() {
            let mut start: Option<AddRec> = None;
            let mut step: Option<i64> = None;
            let mut ok = true;
            for &q in &c.preds[self.header as usize] {
                let arg = match self.edge_arg(f, q, self.header, k) {
                    Some(a) => a,
                    None => {
                        ok = false;
                        break;
                    }
                };
                if !self.inloop[q as usize] {
                    // an entry edge: the start value, which must be invariant
                    let arg = self.const_through(f, c, q, arg, 4).unwrap_or(arg);
                    match self.leaf(f, arg) {
                        Some(a) if a.is_invariant() && start.is_none() => start = Some(a),
                        // two entry edges disagreeing is not one recurrence
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                } else {
                    match self.step_of(f, arg, p) {
                        Some(d) if step.is_none() || step == Some(d) => step = Some(d),
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
            }
            if let (true, Some(a), Some(d)) = (ok, start, step) {
                self.ivs.insert(
                    p,
                    AddRec { base: a.base, off: a.off, step: d, ty: f.ty_of(p) },
                );
            }
        }
    }

    /// The literal `o` denotes on entry to `blk`, seeing through block
    /// parameters whose incoming arguments all agree.
    ///
    /// This exists because `licm::preheaders` builds a preheader that FORWARDS
    /// the header's parameters, which turns a literal start into a symbolic one
    /// — and a symbolic start cannot cross a widening (see the `Cvt` rule), so
    /// without this every `for (i = 0; i < n; i++)` would lose its recurrence
    /// the moment a preheader appeared. Replacing the parameter by the constant
    /// every predecessor passes is an equality, not an approximation.
    fn const_through(
        &self,
        f: &Func,
        c: &dom::Cfg,
        blk: BlockId,
        o: Operand,
        fuel: u32,
    ) -> Option<Operand> {
        let v = match o {
            Operand::Val(v) => v,
            k => return Some(k),
        };
        if fuel == 0 {
            return None;
        }
        let k = f.blocks[blk as usize].params.iter().position(|&p| p == v)?;
        if c.preds[blk as usize].is_empty() {
            return None;
        }
        let mut got: Option<Operand> = None;
        for &q in &c.preds[blk as usize] {
            let a = self.edge_arg(f, q, blk, k)?;
            let r = self.const_through(f, c, q, a, fuel - 1)?;
            match got {
                None => got = Some(r),
                Some(x) if x == r => {}
                _ => return None,
            }
        }
        got
    }

    /// `arg` as `p + d` for a literal `d`, when it is.
    fn step_of(&self, f: &Func, arg: Operand, p: ValueId) -> Option<i64> {
        let v = arg.val()?;
        if v == p {
            return Some(0);
        }
        match self.def_inst(f, v)? {
            Inst::Bin { op: BinOp::Add, a, b, .. } => match (*a, *b) {
                (Operand::Val(x), Operand::Imm(k)) if x == p => Some(k),
                (Operand::Imm(k), Operand::Val(x)) if x == p => Some(k),
                _ => None,
            },
            Inst::Bin { op: BinOp::Sub, a, b, .. } => match (*a, *b) {
                (Operand::Val(x), Operand::Imm(k)) if x == p => Some(k.wrapping_neg()),
                _ => None,
            },
            _ => None,
        }
    }

    /// The evolution of `o` in this loop, or `None` when it is outside the
    /// affine fragment. Recursion is bounded: an SSA definition chain cannot
    /// cycle except through a block parameter, which is a leaf here, but the
    /// bound costs nothing and turns a corrupted IR into a refusal rather than a
    /// hang.
    pub fn eval(&self, f: &Func, o: Operand) -> Option<AddRec> {
        // CP2.5 (compile-speed): a per-call memo. `eval_fuel` fans out into BOTH
        // operands of every Add/Sub, so a value shared between the two subtrees
        // was re-evaluated on each path — up to 2^fuel evaluations on a diamond
        // DAG. Caching (value, fuel) within ONE top-level eval collapses that to
        // linear. Scoped to the call, not a struct field, so a change to `ivs`
        // during `analyze` can never return a stale hit: byte-identical.
        let mut memo = HashMap::new();
        self.eval_fuel(f, o, 16, &mut memo)
    }

    fn eval_fuel(
        &self,
        f: &Func,
        o: Operand,
        fuel: u32,
        memo: &mut HashMap<(ValueId, u32), Option<AddRec>>,
    ) -> Option<AddRec> {
        if let Some(a) = self.leaf(f, o) {
            return Some(a);
        }
        if fuel == 0 {
            return None;
        }
        let v = o.val()?;
        if let Some(a) = self.ivs.get(&v) {
            return Some(*a);
        }
        if let Some(cached) = memo.get(&(v, fuel)) {
            return *cached;
        }
        let r = self.eval_step(f, v, fuel, memo);
        memo.insert((v, fuel), r);
        r
    }

    fn eval_step(
        &self,
        f: &Func,
        v: ValueId,
        fuel: u32,
        memo: &mut HashMap<(ValueId, u32), Option<AddRec>>,
    ) -> Option<AddRec> {
        let ty = f.ty_of(v);
        match self.def_inst(f, v)? {
            Inst::Bin { op: BinOp::Add, a, b, .. } => {
                let (x, y) =
                    (self.eval_fuel(f, *a, fuel - 1, memo)?, self.eval_fuel(f, *b, fuel - 1, memo)?);
                // At most one symbolic term: `base1 + base2` is not in the form.
                let base = match (x.base, y.base) {
                    (None, b) => b,
                    (b, None) => b,
                    _ => return None,
                };
                Some(AddRec {
                    base,
                    off: x.off.wrapping_add(y.off),
                    step: x.step.wrapping_add(y.step),
                    ty,
                })
            }
            Inst::Bin { op: BinOp::Sub, a, b, .. } => {
                let (x, y) =
                    (self.eval_fuel(f, *a, fuel - 1, memo)?, self.eval_fuel(f, *b, fuel - 1, memo)?);
                // Subtracting a symbolic term would need it negated, which the
                // form has no room for.
                if y.base.is_some() {
                    return None;
                }
                Some(AddRec {
                    base: x.base,
                    off: x.off.wrapping_sub(y.off),
                    step: x.step.wrapping_sub(y.step),
                    ty,
                })
            }
            Inst::Bin { op: BinOp::Mul, a, b, .. } => {
                let (x, k) = match (*a, *b) {
                    (p, Operand::Imm(k)) => (self.eval_fuel(f, p, fuel - 1, memo)?, k),
                    (Operand::Imm(k), p) => (self.eval_fuel(f, p, fuel - 1, memo)?, k),
                    _ => return None,
                };
                self.scale(x, k, ty)
            }
            // `x << k` is `x * 2^k` for every shift the type admits; a shift at
            // or past the width is undefined (C99 6.5.7p3), so it is refused
            // rather than folded to zero.
            Inst::Bin { op: BinOp::Shl, a, b, .. } => match *b {
                Operand::Imm(k) if k >= 0 && (k as u32) < ty.bits() => {
                    let x = self.eval_fuel(f, *a, fuel - 1, memo)?;
                    self.scale(x, 1i64 << k, ty)
                }
                _ => None,
            },
            // A widening conversion is the identity — and therefore carries the
            // recurrence — only while the narrow value never wraps. See
            // `stays_in_range`; without a trip count the answer is `None`, which
            // is why `p[i]` for an `int i` is invisible to this analysis in a
            // loop whose bound is unknown. That is the honest answer, not a
            // limitation to route around.
            Inst::Cvt { op: op @ (CvtOp::Sext | CvtOp::Zext), from, a, .. } => {
                let signed = matches!(op, CvtOp::Sext);
                let x = self.eval_fuel(f, *a, fuel - 1, memo)?;
                // Two independent licences, and either suffices. The exit test
                // bounds the counter by its TYPE (`find_nowrap`) — which works
                // with a symbolic bound and is the one that fires in real code —
                // or the trip count bounds it by ARITHMETIC (`stays_in_range`),
                // which works when the counter is not the one being tested.
                let bounded_by_test = a.val().is_some_and(|v| {
                    let set = if signed { &self.nowrap_signed } else { &self.nowrap_unsigned };
                    set.contains(&v)
                });
                if !bounded_by_test && !self.stays_in_range(&x, from.bits(), signed) {
                    return None;
                }
                // A SYMBOLIC start cannot cross the conversion. `sext(k + n)` is
                // not `k + n` in the wider type unless `k`'s own extension is
                // known, and this form has no room to say so. Refusing is the
                // only correct answer — and dropping the base instead was a real
                // miscompile: `for (i = k; …) p[i]` came back as `{p + 0, +, 4}`,
                // so pointer-IV started the walk at `p` rather than at `p + k*4`
                // and insertion sort returned the wrong array (§13h).
                if x.base.is_some() {
                    return None;
                }
                Some(AddRec { base: None, off: x.off, step: x.step, ty })
            }
            _ => None,
        }
    }

    /// Multiplying a symbolic base is not in the form, so only a constant-based
    /// recurrence scales.
    fn scale(&self, x: AddRec, k: i64, ty: Ty) -> Option<AddRec> {
        if x.base.is_some() {
            return None;
        }
        Some(AddRec {
            base: None,
            off: x.off.wrapping_mul(k),
            step: x.step.wrapping_mul(k),
            ty,
        })
    }

    /// A constant or a loop-invariant value: step 0, nothing to unfold.
    fn leaf(&self, f: &Func, o: Operand) -> Option<AddRec> {
        match o {
            Operand::Imm(k) => Some(AddRec { base: None, off: k, step: 0, ty: Ty::I64 }),
            Operand::Val(v) if self.is_loop_invariant(f, v) => Some(AddRec {
                base: Some(v),
                off: 0,
                step: 0,
                ty: f.ty_of(v),
            }),
            _ => None,
        }
    }

    fn def_inst<'a>(&self, f: &'a Func, v: ValueId) -> Option<&'a Inst> {
        match f.values[v as usize].def {
            Def::Inst(b, i) => f.blocks[b as usize].insts.get(i as usize),
            _ => None,
        }
    }

    fn edge_arg(&self, f: &Func, from: BlockId, to: BlockId, k: usize) -> Option<Operand> {
        let mut found = None;
        for t in f.blocks[from as usize].term.targets() {
            if t.block == to {
                let a = *t.args.get(k)?;
                // Two edges from the same block passing different values is not
                // one recurrence.
                match found {
                    None => found = Some(a),
                    Some(x) if x == a => {}
                    _ => return None,
                }
            }
        }
        found
    }

    /// How many times the BODY runs, when that is a number this analysis can
    /// state exactly.
    ///
    /// Claimed only for a loop with ONE exit, whose test compares an induction
    /// variable against a literal bound from a literal start, with the step
    /// moving towards it. Everything else is `None`: a trip count wrong by one is
    /// worse than absent, because final-value and LFTR write it into the program.
    ///
    /// WHERE THE TEST SITS is part of the answer, and it is the part that is easy
    /// to get wrong. Let `k` be the number of times the test says "stay in".
    /// A TOP-tested loop asks before each body execution, so the body runs `k`
    /// times. A BOTTOM-tested one — which is every counted loop here now that
    /// rotation ships — asks after, so the body runs `k + 1`. The two shapes are
    /// distinguished by whether the exiting block is the header and the header is
    /// not itself a latch, and both are pinned by battery.
    fn compute_trips(&self, f: &Func) -> Option<u64> {
        let mut exiting = None;
        for b in 0..f.blocks.len() as BlockId {
            if !self.inloop[b as usize] {
                continue;
            }
            if f.blocks[b as usize]
                .term
                .succs()
                .iter()
                .any(|&s| !self.inloop[s as usize])
            {
                if exiting.is_some() {
                    return None; // more than one way out
                }
                exiting = Some(b);
            }
        }
        let ex = exiting?;
        // A test in the MIDDLE of the body counts different halves differently;
        // there is no single trip count to report.
        //
        // TOP vs BOTTOM IS A DATA-FLOW QUESTION, NOT A PLACEMENT ONE — the same
        // lesson `rotate.rs` records about its own termination argument, and it
        // was learned here the same way. This used to read "the exiting block is
        // the header and the header is not a latch", which is true of a
        // top-tested loop and ALSO true of the shape rotation now produces:
        // rotation puts the body into the header and `cfg::merge` then absorbs
        // the latch, leaving one block that does the work and THEN tests. That
        // block is the header, and the latch is the empty back-edge block, so
        // the placement test said "top" for a loop whose test is at the bottom
        // and the count came out one short (10 iterations reported as 9,
        // battery `scev_counts_the_trips_of_a_literal_loop`).
        //
        // What actually distinguishes them: a TOP test executes before any body
        // work, so the exiting block computes NOTHING but the condition — every
        // instruction in it lies in the condition's transitive cone. The moment
        // one does not, that instruction is body work performed before the test,
        // and the body has run once more than the test has said "stay".
        let top = ex == self.header
            && !self.latches.contains(&self.header)
            && block_is_only_the_test(f, ex);
        // A bottom test must be the LAST thing an iteration does: either it sits
        // at a latch, or it sits in the header itself with the body above it
        // (rotation's shape, once `cfg::merge` has absorbed the latch). In both
        // the exiting block runs exactly once per iteration, which is what makes
        // "body executions = k + 1" true.
        if !top && !(self.latches.contains(&ex) || ex == self.header) {
            return None;
        }
        let (cond, taken_in) = match &f.blocks[ex as usize].term {
            Term::Br(cond, t, e) => {
                match (self.inloop[t.block as usize], self.inloop[e.block as usize]) {
                    (true, false) => (*cond, true),
                    (false, true) => (*cond, false),
                    _ => return None,
                }
            }
            _ => return None,
        };
        let (op, a, b) = match self.def_inst(f, cond.val()?)? {
            Inst::Cmp { op, a, b, .. } => (*op, *a, *b),
            _ => return None,
        };
        // Normalize to "stay in the loop while LHS <op> RHS".
        let op = if taken_in { op } else { invert(op)? };
        let (iv, bound) = (self.eval(f, a)?, self.eval(f, b)?);
        let (iv, bound, op) = if iv.step != 0 { (iv, bound, op) } else { (bound, iv, swap(op)?) };
        if iv.step == 0 || bound.step != 0 || iv.base.is_some() || bound.base.is_some() {
            return None;
        }
        let (start, limit, step) = (iv.off as i128, bound.off as i128, iv.step as i128);
        // The exclusive end of the counting range, as the comparison states it.
        let end = match (op, step > 0) {
            (CmpOp::Slt | CmpOp::Ult, true) => limit,
            (CmpOp::Sle | CmpOp::Ule, true) => limit + 1,
            (CmpOp::Sgt | CmpOp::Ugt, false) => limit,
            (CmpOp::Sge | CmpOp::Uge, false) => limit - 1,
            // `!=` counts only when the step divides the distance exactly and
            // points at it; otherwise the counter steps straight past and the
            // loop is unbounded, which is not a trip count.
            (CmpOp::Ne, _) => {
                let d = limit - start;
                if d != 0 && (d % step != 0 || d.signum() != step.signum()) {
                    return None;
                }
                limit
            }
            _ => return None,
        };
        let span = end - start;
        let k: i128 = if span <= 0 { 0 } else { (span + step.abs() - 1) / step.abs() };
        // The counter must reach `end` without leaving its own type; one that
        // wraps never compares as this arithmetic assumed.
        let bits = iv.ty.bits();
        let (lo, hi) = (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1);
        if start < lo || end > hi {
            return None;
        }
        u64::try_from(if top { k } else { k + 1 }).ok()
    }
}

/// Does this block compute nothing but its own branch condition? Every
/// instruction must lie in the transitive cone of the terminator's condition;
/// one that does not is body work performed before the test.
fn block_is_only_the_test(f: &Func, b: BlockId) -> bool {
    let blk = &f.blocks[b as usize];
    let cond = match &blk.term {
        Term::Br(c, ..) | Term::Switch(c, ..) => *c,
        _ => return false,
    };
    let mut at: std::collections::HashMap<ValueId, usize> = std::collections::HashMap::new();
    for (i, inst) in blk.insts.iter().enumerate() {
        if let Some(d) = inst.dst() {
            at.insert(d, i);
        }
    }
    let mut keep = vec![false; blk.insts.len()];
    let mut work: Vec<ValueId> = cond.val().into_iter().collect();
    while let Some(v) = work.pop() {
        let Some(&i) = at.get(&v) else { continue };
        if keep[i] {
            continue;
        }
        keep[i] = true;
        blk.insts[i].uses(|o| {
            if let Operand::Val(x) = o {
                work.push(x);
            }
        });
    }
    keep.iter().all(|&x| x)
}

fn invert(op: CmpOp) -> Option<CmpOp> {
    Some(match op {
        CmpOp::Eq => CmpOp::Ne,
        CmpOp::Ne => CmpOp::Eq,
        CmpOp::Slt => CmpOp::Sge,
        CmpOp::Sle => CmpOp::Sgt,
        CmpOp::Sgt => CmpOp::Sle,
        CmpOp::Sge => CmpOp::Slt,
        CmpOp::Ult => CmpOp::Uge,
        CmpOp::Ule => CmpOp::Ugt,
        CmpOp::Ugt => CmpOp::Ule,
        CmpOp::Uge => CmpOp::Ult,
        _ => return None,
    })
}

/// The same relation with its operands exchanged.
fn swap(op: CmpOp) -> Option<CmpOp> {
    Some(match op {
        CmpOp::Eq => CmpOp::Eq,
        CmpOp::Ne => CmpOp::Ne,
        CmpOp::Slt => CmpOp::Sgt,
        CmpOp::Sle => CmpOp::Sge,
        CmpOp::Sgt => CmpOp::Slt,
        CmpOp::Sge => CmpOp::Sle,
        CmpOp::Ult => CmpOp::Ugt,
        CmpOp::Ule => CmpOp::Uge,
        CmpOp::Ugt => CmpOp::Ult,
        CmpOp::Uge => CmpOp::Ule,
        _ => return None,
    })
}

/// The block that falls into `header` from outside the loop, when there is
/// exactly one such edge and its source has this header as its only successor.
/// Identical to `licm`'s rule, deliberately: a consumer of this analysis places
/// code in the preheader licm would have placed it in.
fn preheader_of(f: &Func, c: &dom::Cfg, dt: &dom::DomTree, header: BlockId) -> Option<BlockId> {
    let outside: Vec<BlockId> = c.preds[header as usize]
        .iter()
        .copied()
        .filter(|&p| !dt.dominates(header, p))
        .collect();
    match outside.as_slice() {
        [p] if c.succs[*p as usize].len() == 1 => Some(*p),
        _ => None,
    }
}
