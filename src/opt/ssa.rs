// src/opt/ssa.rs — SSA form — SROA, construction (to_ssa), destruction (out_of_ssa).
// One optimization family per file (see opt/mod.rs). Semantics-preservation is
// proved in opt::tests via the commuting square; a pure code-move leaves the
// emitted .s byte-identical (determinism seal).

use super::*;

/// Record the access type of a local; a second, DIFFERENT type (type punning via a
/// union / reinterpretation) makes it non-promotable (an SSA value has one type).
pub(crate) fn note_ty(ty_of: &mut HashMap<u32, TypeId>, escaped: &mut HashSet<u32>, off: u32, ty: TypeId) {
    match ty_of.get(&off) {
        Some(&prev) if prev != ty => {
            escaped.insert(off);
        }
        None => {
            ty_of.insert(off, ty);
        }
        _ => {}
    }
}


/// What to do with one instruction during the fill walk (computed WITHOUT holding a
/// borrow of the instruction, so the `Keep` arm can move it).
pub(crate) enum Act {
    Drop,                     // a dead Lea of a promoted local
    Keep,                     // untouched
    Store(usize, Val),        // writeVariable(var, block, val); delete the Store
    Load(Tmp, TypeId, usize), // dst = readVariable(var, block); Load → Copy(dst, ty, ·)
}


/// Braun's incremental-construction state (per function).
pub(crate) struct Ssa {
    var_ty: Vec<TypeId>,                     // [var] → the local's scalar type (φ / Copy type)
    current_def: Vec<HashMap<BlockId, Val>>, // [var][block] → the reaching value (Braun's currentDef)
    sealed: Vec<bool>,                       // [block] → all predecessors known?
    incomplete: Vec<Vec<(usize, Tmp)>>,      // [block] → (var, φ) awaiting operands until sealed
    preds: Vec<Vec<BlockId>>,
    phi_block: HashMap<Tmp, BlockId>,       // φ temp → the block it heads
    phi_var: HashMap<Tmp, usize>,           // φ temp → the variable it reconciles
    phi_arms: HashMap<Tmp, Vec<(BlockId, Val)>>, // φ temp → [(pred, value)]
    base: u32,                              // first fresh temp id (= |temps| before construction)
    new_temps: Vec<TypeId>,                 // types of the φ temps appended to Γ
}


impl Ssa {
    fn new_temp(&mut self, ty: TypeId) -> Tmp {
        let t = self.base + self.new_temps.len() as u32;
        self.new_temps.push(ty);
        t
    }
    fn new_phi(&mut self, var: usize, block: BlockId) -> Tmp {
        let ty = self.var_ty[var];
        let t = self.new_temp(ty);
        self.phi_block.insert(t, block);
        self.phi_var.insert(t, var);
        self.phi_arms.insert(t, Vec::new());
        t
    }
    fn write_var(&mut self, var: usize, block: BlockId, val: Val) {
        self.current_def[var].insert(block, val);
    }
    /// readVariable (Braun §2): the value of `var` reaching the START-or-here of
    /// `block`, following the local definition first, else recursing over the CFG.
    fn read_var(&mut self, var: usize, block: BlockId) -> Val {
        if let Some(v) = self.current_def[var].get(&block) {
            return *v;
        }
        self.read_var_recursive(var, block)
    }
    fn read_var_recursive(&mut self, var: usize, block: BlockId) -> Val {
        let val = if !self.sealed[block as usize] {
            // CFG still incomplete here: place an operandless φ, filled at seal time.
            let phi = self.new_phi(var, block);
            self.incomplete[block as usize].push((var, phi));
            Val::Tmp(phi)
        } else if self.preds[block as usize].is_empty() {
            // Undefined read: the recursion reached a block with NO predecessor (the entry,
            // or an unreachable block) without finding a definition — the variable is read
            // before any write on this path. The C program has UB (C99 6.3.2.1p2: an
            // indeterminate value of an object whose address is never taken). Building a φ
            // here would be malformed (a φ needs a predecessor edge) ⟹ broken IR. Any value
            // is permissible under the UB, so materialize a deterministic, well-formed 0
            // (as LLVM lowers `undef`). GCC torture pr43629.
            Val::Imm(0)
        } else if self.preds[block as usize].len() == 1 {
            let p = self.preds[block as usize][0];
            self.read_var(var, p) // no join ⟹ no φ needed (minimal SSA)
        } else {
            // ≥2 predecessors: a φ is required. Write it FIRST to break loops.
            let phi = self.new_phi(var, block);
            self.write_var(var, block, Val::Tmp(phi));
            self.add_phi_operands(var, phi)
        };
        self.write_var(var, block, val);
        val
    }
    fn add_phi_operands(&mut self, var: usize, phi: Tmp) -> Val {
        let block = self.phi_block[&phi];
        for p in self.preds[block as usize].clone() {
            let v = self.read_var(var, p);
            self.phi_arms.get_mut(&phi).unwrap().push((p, v));
        }
        Val::Tmp(phi)
    }
    fn seal(&mut self, block: BlockId) {
        if self.sealed[block as usize] {
            return;
        }
        for (var, phi) in std::mem::take(&mut self.incomplete[block as usize]) {
            self.add_phi_operands(var, phi);
        }
        self.sealed[block as usize] = true;
    }
}


/// CFG-completeness precondition (shared by every dominance/reachability-based pass).
/// A computed goto (`GotoPtr`, EXT gcc) transfers to a data-dependent address-taken
/// label; its edges are NOT modeled by any block terminator (the IR block after it is a
/// dead-end `Ret`). So `predecessors`/`rpo`/`dominators`/reachability see an INCOMPLETE
/// CFG — e.g. a loop closed only by `goto *p` looks acyclic. Any transform that trusts the
/// CFG (mem2reg φ-placement, GVN dominance, SCCP reachability) is then UNSOUND. Passes
/// bail on such a function ⟹ identity transform, leaving it for the naive -O0 backend.
/// GCC torture 920302-1, 920501-3 (mem2reg dropped a loop φ / GVN pruned a live block →
/// wild GotoPtr → SIGSEGV).
pub(crate) fn cfg_complete(f: &IrFunc) -> bool {
    !f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::GotoPtr(..))))
}


// ─────────────────────────────────────────────────────────────────────────────
// SROA — Scalar Replacement of Aggregates.  An IR→IR pass that runs AFTER inlining
// and BEFORE to_ssa.  A struct/array field access is lowered as an ADDRESS COMPUTATION
// `Lea(Local(base)) (+ Add const)*` feeding a scalar Load/Store; the base Lea escapes
// via the Add, so to_ssa (which only promotes offsets accessed through a bare Lea(Local))
// leaves the whole aggregate in the frame.  SROA rewrites each constant-offset field
// address into a SINGLE `Lea(Local(base − off))` — the identical byte, but now a bare
// Lea(Local(_)) that to_ssa promotes to an SSA scalar.  This is the lever for struct-heavy
// code: fields become registers instead of frame memory.
//
// WHY an IR pass after inline, NOT a lowering-time fold: after `bump(&q)` is inlined, the
// callee body reaches `q.f` through the SAME substituted base tmp, so ALL accesses (the
// caller's Member accesses and the inlinee's pointer arithmetic) go through one base Lea and
// are rewritten UNIFORMLY — no aliasing split.  A lowering-time fold sees only the caller's
// Member syntax and would rewrite half, leaving the inlined pointer half unfolded → a store
// forwarded across an aliasing write (miscompile).  Doing it here is why compilers put SROA
// on a mid-level IR with escape analysis.
//
// SOUNDNESS (the decomposability gate).  A base local is split ONLY if:
//   (1) every use of any address tmp of it is a Load/Store ADDRESS or the operand of a
//       resolved constant `Add` — never stored as a value, passed to a call, compared, or
//       returned (⟹ no pointer escapes to alias its storage), and
//   (2) no two DISTINCT field offsets partially overlap (unions / cast-punning): overlapping
//       fields would be promoted to independent SSA scalars yet share bytes → miscompile.
// (Same-offset type conflicts are left to to_ssa's note_ty, which escapes that one offset.)
//
// GOVERNING THEOREM (CbC): ⟦f⟧ = ⟦sroa(f)⟧.  Each rewritten address is value-identical —
// `⟦Lea(Local(base−c))⟧ = frame − (base−c) = (frame − base) + c = ⟦Add(Lea(Local(base)), c)⟧`
// (Lea interp, ir.rs) — and the gate guarantees no other access reaches the storage, so
// splitting one slot into per-field slots preserves the memory model.  MEASURED by `equiv`.
// Pre-to_ssa the IR is single-def per temp (each lowered value gets a fresh temp), so an
// address tmp resolves to one (base, offset).
pub fn sroa(tt: &TyTab, f: &mut IrFunc) -> u32 {
    // 1. Resolve local-address tmps to (base, resolved local offset). Lea(Local) seeds;
    //    `Add(tmp, +const)` extends. Fixpoint (an Add may textually precede its base's Lea).
    let mut addr: HashMap<Tmp, (u32, u32)> = HashMap::new();
    loop {
        let mut changed = false;
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Lea(t, Place::Local(off)) if !addr.contains_key(t) => {
                        addr.insert(*t, (*off, *off));
                        changed = true;
                    }
                    Inst::Bin(t, Op::Add, _, x, y) if !addr.contains_key(t) => {
                        let one = match (x, y) {
                            (Val::Tmp(s), Val::Imm(c)) | (Val::Imm(c), Val::Tmp(s)) => Some((*s, *c)),
                            _ => None,
                        };
                        if let Some((s, c)) = one
                            && c >= 0
                            && let Some(&(base, roff)) = addr.get(&s)
                            && let Some(nr) = roff.checked_sub(c as u32)
                        {
                            addr.insert(*t, (base, nr));
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        if !changed {
            break;
        }
    }
    if addr.is_empty() {
        return 0;
    }
    let base_of = |t: Tmp| addr.get(&t).map(|&(b, _)| b);

    // 2. Escape / decomposability. A base is BAD if any address tmp of it is used as anything
    //    other than a Load/Store address or the operand of a resolved const-Add.
    // A PARAMETER's storage is ABI-delivered into its frame slot as a WHOLE unit (emit_params),
    // not by in-body Stores. Splitting a struct param's fields into distinct offsets would make
    // to_ssa promote offsets it cannot seed from the ABI delivery (no def, not a param_off) →
    // an undefined read (931004-5, 20000707-1: struct-by-value args). Never split a param base.
    let mut bad: HashSet<u32> = f.params.iter().map(|&(off, _)| off).collect();
    let mut buf = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                // A VOLATILE field access (C99 6.7.3) must stay in memory — mark its base BAD
                // so the aggregate is never scalarized (a promoted field would be register-cached).
                Inst::Load(_, ty, Val::Tmp(a)) => {
                    if tt.is_volatile(*ty) && let Some(bb) = base_of(*a) {
                        bad.insert(bb);
                    }
                }
                Inst::Bin(d, Op::Add, _, _, _) if addr.contains_key(d) => {} // field-Add — ok
                Inst::Store(ty, addr_v, val) => {
                    if tt.is_volatile(*ty)
                        && let Val::Tmp(a) = addr_v
                        && let Some(bb) = base_of(*a)
                    {
                        bad.insert(bb);
                    }
                    // the address (2nd operand) is a fine use; a stored VALUE that is an
                    // address tmp is a pointer escaping into memory → its base is BAD.
                    if let Val::Tmp(v) = val
                        && let Some(bb) = base_of(*v)
                    {
                        bad.insert(bb);
                    }
                }
                other => {
                    buf.clear();
                    inst_uses(other, &mut buf);
                    for &u in &buf {
                        if let Some(bb) = base_of(u) {
                            bad.insert(bb);
                        }
                    }
                }
            }
        }
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            if let Some(bb) = base_of(u) {
                bad.insert(bb);
            }
        }
    }

    // 3. Partial-overlap guard: distinct field offsets whose byte ranges intersect (union /
    //    cast punning) cannot both be promoted to independent scalars.
    let mut acc: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    for b in &f.blocks {
        for i in &b.insts {
            let (u, ty) = match i {
                Inst::Load(_, ty, Val::Tmp(u)) => (*u, *ty),
                Inst::Store(ty, Val::Tmp(u), _) => (*u, *ty),
                _ => continue,
            };
            if let Some(&(base, roff)) = addr.get(&u) {
                acc.entry(base).or_default().push((roff, tt.size(ty)));
            }
        }
    }
    for (&base, list) in &acc {
        'pair: for a in 0..list.len() {
            for c in (a + 1)..list.len() {
                let ((o1, s1), (o2, s2)) = (list[a], list[c]);
                if o1 == o2 {
                    continue; // same offset ⟹ note_ty in to_ssa arbitrates the type
                }
                // address of a field at local offset o = frame − o; interval [−o, −o+size).
                let (lo1, hi1) = (-(o1 as i64), -(o1 as i64) + s1 as i64);
                let (lo2, hi2) = (-(o2 as i64), -(o2 as i64) + s2 as i64);
                if lo1 < hi2 && lo2 < hi1 {
                    bad.insert(base);
                    break 'pair;
                }
            }
        }
    }

    // 4. Rewrite each address tmp of a GOOD base to a single folded Lea(Local(resolved)).
    let mut changed = 0u32;
    for b in &mut f.blocks {
        for i in &mut b.insts {
            let resolved = match i {
                Inst::Lea(t, Place::Local(_)) | Inst::Bin(t, Op::Add, _, _, _) => addr.get(t).copied(),
                _ => None,
            };
            let Some((base, roff)) = resolved else { continue };
            if bad.contains(&base) {
                continue;
            }
            if let Inst::Lea(_, Place::Local(off)) = i
                && *off == roff
            {
                continue; // already the folded form (a base Lea at offset 0)
            }
            let t = match i {
                Inst::Lea(t, _) | Inst::Bin(t, ..) => *t,
                _ => unreachable!(),
            };
            *i = Inst::Lea(t, Place::Local(roff));
            changed += 1;
        }
    }
    changed
}


pub fn to_ssa(tt: &TyTab, f: &mut IrFunc) {
    if !cfg_complete(f) {
        return;
    }
    // ── 1. Promotability analysis ────────────────────────────────────────────
    // A parameter's value is written to its ABI frame slot by emit_params (prologue),
    // never by a Store in the body — so its promotable var has no in-body def to seed φ.
    // We PROMOTE a non-address-taken scalar param by materializing its slot value ONCE at
    // entry (an entry Load = the readback of the ABI spill) and threading it as SSA — this
    // eliminates the per-use reloads. Address-taken params are still caught as escaped by
    // the general Lea-escape scan below (a Lea used as a value ⟹ escape), so they stay in
    // memory. `param_off` marks which promotable offsets need the entry seed; `addr_ty`
    // records a pointer type per offset (reused for the seed Lea's dst).
    let param_off: HashSet<u32> = f.params.iter().map(|&(off, _)| off).collect();
    // SUB-WORD params (char/short, size < 4) stay in memory: a promoted sub-word value
    // carried around a loop is not re-canonicalized to its width on the back-edge (the
    // narrowing Store that a sub-word LOCAL still carries is gone), so its wrap (mod 2^8 /
    // 2^16) is lost — a miscompile (pr81913: `u8 d; d--; while(d>=(u8)e)` never wraps at
    // 256). Full-width scalars (int/long/ptr/float/double) wrap at the register width itself
    // and are safe. Sub-word params are rare; the int/long/ptr bulk keeps the win.
    let mut escaped: HashSet<u32> =
        f.params.iter().filter(|&&(_, t)| tt.size(t) < 4).map(|&(off, _)| off).collect();
    let mut lea_off: HashMap<Tmp, u32> = HashMap::new();
    let mut addr_ty: HashMap<u32, TypeId> = HashMap::new();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Lea(t, Place::Local(off)) = i {
                lea_off.insert(*t, *off);
                addr_ty.entry(*off).or_insert(f.temps[*t as usize]);
            }
        }
    }
    // An offset escapes if any Lea of it is used other than as a Load/Store address;
    // its type is the (single) type of its scalar accesses.
    let mut ty_of: HashMap<u32, TypeId> = HashMap::new();
    let mut has_mem: HashSet<u32> = HashSet::new();
    let mut uses = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                // A volatile local access (C99 6.7.3) ESCAPES its offset: the value must
                // stay in memory (never promoted to an SSA scalar, which would cache it).
                Inst::Load(_, ty, Val::Tmp(a)) => {
                    if let Some(&off) = lea_off.get(a) {
                        note_ty(&mut ty_of, &mut escaped, off, *ty);
                        if tt.is_volatile(*ty) {
                            escaped.insert(off);
                        }
                        has_mem.insert(off);
                    }
                }
                Inst::Store(ty, Val::Tmp(a), _) => {
                    if let Some(&off) = lea_off.get(a) {
                        note_ty(&mut ty_of, &mut escaped, off, *ty);
                        if tt.is_volatile(*ty) {
                            escaped.insert(off);
                        }
                        has_mem.insert(off);
                    }
                }
                _ => {}
            }
            uses.clear();
            inst_uses(i, &mut uses);
            for &u in &uses {
                if let Some(&off) = lea_off.get(&u) {
                    if !is_addr_use(i, u) {
                        escaped.insert(off);
                    }
                }
            }
        }
        uses.clear();
        term_uses(&b.term, &mut uses);
        for &u in &uses {
            if let Some(&off) = lea_off.get(&u) {
                escaped.insert(off);
            }
        }
    }
    // The promotable set: scalar (int/float/pointer, per LP64 TyTab), has real
    // memory traffic, not escaped. A dense var index gives φ/currentDef arrays.
    // A PARAMETER offset carries a second constraint: its DECLARED type (f.params) must
    // itself be a GP/FP scalar. A composite param (e.g. `struct{ull a:16,b:32,c:16}` — 8B)
    // is accessed through integer-typed bitfield RMW, so `ty_of[off]` is integer and would
    // pass the traffic-type `scalar` test — but the ABI delivers it as a STRUCT (emit_params'
    // struct arm, no param_loc), so promoting it to an Inst::Param would mismatch (param_loc
    // None → the `unreachable!` at emit time, gcc 20081117-1). Gate param offsets on the
    // declared type too.
    let param_decl: HashMap<u32, TypeId> = f.params.iter().map(|&(o, t)| (o, t)).collect();
    let is_scalar_ty = |ty: TypeId| {
        tt.is_integer(ty) || tt.is_float(ty) || matches!(tt.tys[ty as usize], Ty::Ptr(_))
    };
    let mut promotable: Vec<u32> = ty_of
        .iter()
        .filter_map(|(&off, &ty)| {
            let scalar = is_scalar_ty(ty)
                && param_decl.get(&off).map_or(true, |&pt| is_scalar_ty(pt));
            (!escaped.contains(&off) && has_mem.contains(&off) && scalar).then_some(off)
        })
        .collect();
    if promotable.is_empty() {
        return; // no promotion possible ⟹ identity transform (zero perf change)
    }
    promotable.sort_unstable();
    let off2var: HashMap<u32, usize> = promotable.iter().enumerate().map(|(i, &o)| (o, i)).collect();
    let var_ty: Vec<TypeId> = promotable.iter().map(|o| ty_of[o]).collect();

    let nb = f.blocks.len();
    let mut s = Ssa {
        var_ty,
        current_def: vec![HashMap::new(); promotable.len()],
        sealed: vec![false; nb],
        incomplete: vec![Vec::new(); nb],
        preds: predecessors(f),
        phi_block: HashMap::new(),
        phi_var: HashMap::new(),
        phi_arms: HashMap::new(),
        base: f.temps.len() as u32,
        new_temps: Vec::new(),
    };

    // ── 1b. Seed promoted PARAMS. A param var has no in-body def; without a seed, a read at
    // the entry (preds-empty) would resolve to Imm(0) (read_var_recursive) — a miscompile.
    // Materialize the slot's ABI-spilled value once at entry (Lea+Load, kept by the fill
    // loop via `seed_addr`), and seed currentDef[var][entry] with it. A later Store in the
    // body overrides normally; a param reassigned before first read leaves this Load dead
    // (DCE). Locals are NOT seeded — their own Store is the def.
    const ENTRY: BlockId = 0;
    let off2idx: HashMap<u32, u32> =
        f.params.iter().enumerate().map(|(i, &(off, _))| (off, i as u32)).collect();
    let mut seed_addr: HashSet<Tmp> = HashSet::new();
    // Two seed groups with a HARD ordering constraint. A GP Param reads an incoming argument
    // register (x0–x7); an FP seed's Lea+Load uses x0/x9 as scratch. So EVERY GP Param must be
    // delivered BEFORE any FP seed, else an FP seed clobbers an arg register a later Param
    // still needs (20020413-1: `test(long double, int*)` — the val-seed's `mov x0,..` wiped
    // eval's x0). GP Params are ordered by argument index so the x0-funnel of a spilled Param
    // only ever clobbers an argument register already delivered.
    let mut gp_params: Vec<(u32, Tmp)> = Vec::new(); // (arg index, temp)
    let mut fp_seeds: Vec<Inst> = Vec::new();
    for (vi, &off) in promotable.iter().enumerate() {
        if !param_off.contains(&off) {
            continue;
        }
        let ty = s.var_ty[vi];
        let pt = s.new_temp(ty);
        if tt.is_float(ty) {
            // Step-1 path (FP): materialize the value from the ABI-spilled slot (emit_params
            // still spills FP params; the seed Lea keeps the slot referenced). Delivering an
            // FP arg into a v-home directly is left to a later step.
            let a = s.new_temp(addr_ty[&off]);
            seed_addr.insert(a);
            fp_seeds.push(Inst::Lea(a, Place::Local(off)));
            fp_seeds.push(Inst::Load(pt, ty, Val::Tmp(a)));
        } else {
            // Step-2 path (GP integer/pointer): deliver the incoming arg REGISTER directly
            // into pt's home; emit_params skips the frame spill (the slot goes unreferenced
            // — no Lea — which is exactly the backend's skip criterion). Kills str + reload.
            gp_params.push((off2idx[&off], pt));
        }
        s.write_var(vi, ENTRY, Val::Tmp(pt));
    }
    gp_params.sort_unstable_by_key(|&(i, _)| i);
    let mut seed_insts: Vec<Inst> =
        gp_params.into_iter().map(|(i, pt)| Inst::Param(pt, i)).collect();
    seed_insts.extend(fp_seeds);
    if !seed_insts.is_empty() {
        seed_insts.append(&mut f.blocks[ENTRY as usize].insts);
        f.blocks[ENTRY as usize].insts = seed_insts;
    }

    // ── 2. Fill (RPO) — Store→writeVar (delete), Load→Copy(readVar), dead Lea→drop.
    // Seal a block on entry once all its predecessors are filled (forward joins seal
    // eagerly ⟹ minimal φ); a loop header's back-edge predecessor is still unfilled,
    // so it stays unsealed and its reads create incomplete φ, resolved in step 3.
    let order = rpo(f);
    let mut filled = vec![false; nb];
    for &bi in &order {
        let blk = bi as usize;
        if !s.sealed[blk] && s.preds[blk].iter().all(|&p| filled[p as usize]) {
            s.seal(bi);
        }
        let mut new_insts: Vec<Inst> = Vec::with_capacity(f.blocks[blk].insts.len());
        for inst in std::mem::take(&mut f.blocks[blk].insts) {
            let act = match &inst {
                Inst::Lea(t, Place::Local(off))
                    if off2var.contains_key(off) && !seed_addr.contains(t) =>
                {
                    Act::Drop
                }
                Inst::Store(_, Val::Tmp(a), val)
                    if lea_off.get(a).is_some_and(|o| off2var.contains_key(o)) =>
                {
                    Act::Store(off2var[&lea_off[a]], *val)
                }
                Inst::Load(d, ty, Val::Tmp(a))
                    if lea_off.get(a).is_some_and(|o| off2var.contains_key(o)) =>
                {
                    Act::Load(*d, *ty, off2var[&lea_off[a]])
                }
                _ => Act::Keep,
            };
            match act {
                Act::Drop => {}
                Act::Store(var, val) => s.write_var(var, bi, val),
                Act::Load(d, ty, var) => {
                    let v = s.read_var(var, bi);
                    // A Store into a float(size 4) cell narrows to f32 and the matching
                    // Load widens f32→f64 (ir.rs Store/Load, backend store_narrow / `fcvt
                    // d,s`), so the store∘load round-trip = round-to-f32, NOT identity.
                    // mem2reg elides both, so that narrowing must be restored explicitly —
                    // else the promoted value keeps illegal f64 precision (C99 6.3.1.5).
                    // A self-Cast float→float narrows (eval_cast / backend `fcvt s,d;fcvt
                    // d,s`). Integer cells round-trip as identity (temps are kept canon'd
                    // to their type), so a plain Copy stays faithful there.
                    if tt.is_float(ty) && tt.size(ty) == 4 {
                        new_insts.push(Inst::Cast(d, ty, ty, v));
                    } else {
                        new_insts.push(Inst::Copy(d, ty, v));
                    }
                }
                Act::Keep => new_insts.push(inst),
            }
        }
        f.blocks[blk].insts = new_insts;
        filled[blk] = true;
    }

    // ── 3. Seal any remaining blocks (loop headers) — now every predecessor is
    // filled, so incomplete φ get their operands.
    for bi in 0..nb as BlockId {
        s.seal(bi);
    }

    // ── 4. Extend Γ with the φ temporaries, then materialize Inst::Phi at each
    // block head (deterministic order by temp id).
    f.temps.extend(s.new_temps.iter().copied());
    let mut per_block: Vec<Vec<(Tmp, TypeId, Vec<(BlockId, Val)>)>> = vec![Vec::new(); nb];
    for (&phi, &blk) in &s.phi_block {
        let ty = s.var_ty[s.phi_var[&phi]];
        per_block[blk as usize].push((phi, ty, s.phi_arms[&phi].clone()));
    }
    for blk in 0..nb {
        let mut ps = std::mem::take(&mut per_block[blk]);
        ps.sort_by_key(|(t, _, _)| *t);
        let mut ni: Vec<Inst> =
            ps.into_iter().map(|(t, ty, arms)| Inst::Phi(t, ty, arms)).collect();
        ni.append(&mut f.blocks[blk].insts);
        f.blocks[blk].insts = ni;
    }

    // ── 5. Trivial-φ elimination (Braun §3.1): a φ whose operands (excluding
    // self-references) are one single value V carries V on every edge → replace it
    // by V everywhere and remove it. Semantics-preserving; cascades to a fixpoint.
    remove_trivial_phis(f);
}


/// Remove trivial φs (Braun §3.1). A φ whose non-self arms all resolve to a SINGLE value v
/// collapses to v. The old code re-scanned ALL insts to find one trivial φ then rewrote ALL
/// uses each time — O(#φ · #insts), the yarpgen s0940 compile-time pathology (loop nests
/// spawn thousands of φs). Here triviality is decided against the growing substitution and
/// the whole substitution is applied in ONE final pass. The fixpoint is computed by rounds
/// over the φ set IN PROGRAM ORDER — deterministic, and complete because `subst` grows
/// monotonically until a full round adds nothing (a φ made trivial by a later collapse is
/// caught on the next round; `resolve_subst` chases the collapse chains). ⟦·⟧ is unchanged:
/// the surviving-φ set and the resolved operands are exactly the repeated-scan result,
/// reached without the per-collapse O(#insts) rewrite. (A worklist keyed on φ-users would be
/// asymptotically tighter but must track substitution chains transitively; the round loop is
/// the same fixpoint, inner cost O(#φ) per round with the round count = φ-collapse depth.)
pub(crate) fn remove_trivial_phis(f: &mut IrFunc) {
    let mut phis: Vec<Tmp> = Vec::new(); // deterministic program order
    let mut phi_arms: HashMap<Tmp, Vec<Val>> = HashMap::new();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Phi(d, _, arms) = i {
                phis.push(*d);
                phi_arms.insert(*d, arms.iter().map(|(_, v)| *v).collect());
            }
        }
    }
    let mut subst: HashMap<Tmp, Val> = HashMap::new();
    loop {
        let mut changed = false;
        for &d in &phis {
            if subst.contains_key(&d) {
                continue;
            }
            let mut uniq: Option<Val> = None;
            let mut trivial = true;
            for &v in &phi_arms[&d] {
                let r = resolve_subst(v, &subst);
                if matches!(r, Val::Tmp(t) if t == d) {
                    continue; // self-reference does not count
                }
                match uniq {
                    None => uniq = Some(r),
                    Some(u) => {
                        if !val_eq(u, r) {
                            trivial = false;
                            break;
                        }
                    }
                }
            }
            // uniq==None ⟹ only self-refs (undefined / unreachable): leave the φ in place.
            if trivial && let Some(u) = uniq {
                subst.insert(d, u);
                changed = true;
            }
        }
        if !changed {
            break; // fixpoint: no φ collapsed this round
        }
    }
    if subst.is_empty() {
        return;
    }
    // ONE final pass: drop the collapsed φs; chase every remaining use through `subst`.
    for b in f.blocks.iter_mut() {
        b.insts.retain(|i| !matches!(i, Inst::Phi(d, ..) if subst.contains_key(d)));
        for i in b.insts.iter_mut() {
            each_use_mut(i, |x| *x = resolve_subst(*x, &subst));
        }
        each_use_term_mut(&mut b.term, |x| *x = resolve_subst(*x, &subst));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// OUT-OF-SSA — φ-destruction (Stage 3). The INVERSE of to_ssa's join reconciliation:
// every Inst::Phi is replaced by explicit Inst::Copy on the incoming control edges,
// leaving IR the backend can consume directly (φ is an SSA artifact with no machine
// form — see ir.rs Inst::Phi).
//
// GOVERNING THEOREM (CbC, supreme over the QBE projection): `⟦f⟧ = ⟦out_of_ssa(f)⟧`
// for f in SSA form — MEASURED by `equiv` (translation validation), never trusted.
// Composed with Stage 2 this closes the round trip ⟦to_ssa(f)⟧ = ⟦out_of_ssa(to_ssa(f))⟧.
//
// TWO CLASSIC MISCOMPILE TRAPS (csmith bait), both handled by construction:
//   • critical edges — a φ-block has ≥2 preds; if a predecessor also has ≥2
//     successors, copies appended to it would leak onto its OTHER edge. Such an edge
//     is SPLIT: a fresh block on the edge holds the copies (`split_edge`).
//   • the swap / lost-copy problem — φ-nodes at a block are PARALLEL (simultaneous).
//     Sequentializing {a←b, b←a} naively yields a=b; b=b. `seq_pcopy` orders the
//     copies (a leaf whose dst is read by no pending copy is emitted first) and breaks
//     any remaining cycle by saving one value into a fresh temp.
// ─────────────────────────────────────────────────────────────────────────────


/// Sequentialize a PARALLEL copy set {dst ← src} (dsts distinct) into an ordered list
/// of copies with identical net effect. `fresh(ty)` mints a temp to break cycles.
pub(crate) fn seq_pcopy(pc: &[(Tmp, TypeId, Val)], fresh: &mut impl FnMut(TypeId) -> Tmp) -> Vec<(Tmp, TypeId, Val)> {
    // Identity copies (d ← d) carry no information — drop them.
    let mut pending: Vec<(Tmp, TypeId, Val)> =
        pc.iter().cloned().filter(|(d, _, s)| !matches!(s, Val::Tmp(t) if t == d)).collect();
    let mut out = Vec::new();
    while !pending.is_empty() {
        // A copy is safe to emit now iff its dst is read by no OTHER pending copy
        // (emitting it cannot clobber a value still needed by the parallel set).
        let leaf = pending
            .iter()
            .position(|(d, _, _)| !pending.iter().any(|(d2, _, s)| d2 != d && matches!(s, Val::Tmp(t) if t == d)));
        match leaf {
            Some(i) => out.push(pending.remove(i)),
            None => {
                // All remaining copies form cycles (each dst is read by another): break
                // one by saving the current value of a dst into a fresh temp, then
                // redirect readers to it — the cycle becomes a chain.
                let (d, ty, _) = pending[0];
                let t = fresh(ty);
                out.push((t, ty, Val::Tmp(d))); // t ← d (preserve d's incoming value)
                for (_, _, s) in pending.iter_mut() {
                    if matches!(s, Val::Tmp(x) if *x == d) {
                        *s = Val::Tmp(t);
                    }
                }
            }
        }
    }
    out
}


pub fn out_of_ssa(f: &mut IrFunc) {
    let preds = predecessors(f);
    let succ_cnt: Vec<usize> = successors(f).iter().map(|s| s.len()).collect();

    // Copies to append at the END of a single-successor predecessor (before its term).
    // BTreeMap (not HashMap): the apply loop mints cycle-breaking fresh temps sequentially
    // from a shared counter, so iterating predecessors in hash order would number those
    // temps differently across runs (→ different coloring, different .s). Sorted-by-block-id
    // iteration makes φ-destruction DETERMINISTIC (the out_of_ssa half of the determinism seal).
    let mut append_to: BTreeMap<BlockId, Vec<(Tmp, TypeId, Val)>> = BTreeMap::new();
    // Critical edges to split: (pred, φ-block, the parallel copy set on that edge).
    let mut splits: Vec<(BlockId, BlockId, Vec<(Tmp, TypeId, Val)>)> = Vec::new();

    for b in 0..f.blocks.len() as BlockId {
        // The φ-nodes heading this block (dst, ty, arms), in program order.
        let phis: Vec<(Tmp, TypeId, Vec<(BlockId, Val)>)> = f.blocks[b as usize]
            .insts
            .iter()
            .filter_map(|i| match i {
                Inst::Phi(d, ty, arms) => Some((*d, *ty, arms.clone())),
                _ => None,
            })
            .collect();
        if phis.is_empty() {
            continue;
        }
        // For each DISTINCT predecessor edge, gather the parallel copy set {dst ← arm(P)}.
        let mut seen: HashSet<BlockId> = HashSet::new();
        for &p in &preds[b as usize] {
            if !seen.insert(p) {
                continue; // a multi-edge (Br to the same block twice) — copies are identical
            }
            let pc: Vec<(Tmp, TypeId, Val)> = phis
                .iter()
                .map(|(d, ty, arms)| {
                    let v = arms
                        .iter()
                        .find(|(pp, _)| *pp == p)
                        .map(|(_, v)| *v)
                        .expect("out_of_ssa: φ missing an arm for a predecessor");
                    (*d, *ty, v)
                })
                .collect();
            if succ_cnt[p as usize] == 1 {
                append_to.entry(p).or_default().extend(pc); // safe: p's only edge is p→b
            } else {
                splits.push((p, b, pc)); // critical edge → split
            }
        }
    }

    // Fresh temps for cycle-breaking are appended to Γ.
    let mut new_temps: Vec<TypeId> = Vec::new();
    let base = f.temps.len() as u32;
    let mut fresh = |ty: TypeId| -> Tmp {
        let t = base + new_temps.len() as u32;
        new_temps.push(ty);
        t
    };

    // Apply single-successor appends: insert the sequentialized copies before the term.
    for (p, pc) in append_to {
        let seq = seq_pcopy(&pc, &mut fresh);
        let insts = &mut f.blocks[p as usize].insts;
        for (d, ty, s) in seq {
            insts.push(Inst::Copy(d, ty, s));
        }
    }

    // Apply critical-edge splits: a new block E = {copies; Jmp(b)} on the edge p→b.
    for (p, b, pc) in splits {
        let seq = seq_pcopy(&pc, &mut fresh);
        let insts = seq.into_iter().map(|(d, ty, s)| Inst::Copy(d, ty, s)).collect();
        let e = f.blocks.len() as BlockId;
        f.blocks.push(Block { insts, term: Term::Jmp(b) });
        retarget(&mut f.blocks[p as usize].term, b, e);
    }

    f.temps.extend(new_temps);

    // Every φ has been replaced by edge copies — remove them all.
    for b in f.blocks.iter_mut() {
        b.insts.retain(|i| !matches!(i, Inst::Phi(..)));
    }
}

