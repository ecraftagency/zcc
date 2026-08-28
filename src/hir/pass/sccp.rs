// sccp — Sparse Conditional Constant Propagation (MECHANISM.md §G4 row 3).
// THEORY A7b — optimization: this pass ships its commuting square
//
// Wegman & Zadeck 1991, "Constant Propagation with Conditional Branches". The
// theorem is the reason this pass is not just "fold, then delete dead arms":
// the constant lattice and the REACHABILITY lattice are solved TOGETHER, so a
// value that is constant only because a branch is never taken is still found,
// and an arm that is dead only because a value is constant is still deleted.
// Neither analysis alone is a fixpoint of the other.
//
// Lattice per value: ⊤ (no executable definition seen yet) ⊒ constant ⊒ ⊥.
// The algorithm is OPTIMISTIC — everything starts at ⊤ and only ever descends —
// which is what lets a value that is constant around a loop stay constant.
//
// Commuting square. Two obligations, both local:
//   * A value marked constant equals that constant on every executable run,
//     because the meet is taken over exactly the edges the reachability lattice
//     proved executable, and the transfer function is `fold::fold_inst`, which
//     is ⟦·⟧ itself (see fold.rs).
//   * Replacing `br c, x, y` by `jmp x` when the y-edge is not executable
//     changes no run: no run reaches y.
use super::*;

#[derive(Clone, Copy, PartialEq)]
enum Lat {
    Top,
    Const(Operand),
    Bot,
}

impl Lat {
    fn meet(self, o: Lat) -> Lat {
        match (self, o) {
            (Lat::Top, x) | (x, Lat::Top) => x,
            (Lat::Const(a), Lat::Const(b)) if a == b => Lat::Const(a),
            (Lat::Bot, _) | (_, Lat::Bot) => Lat::Bot,
            _ => Lat::Bot,
        }
    }
}

/// THEORY A7b  SQUARE sccp_kills_the_arm_a_constant_makes_unreachable — the lattice meet
pub fn run(f: &mut Func) -> bool {
    let n = f.blocks.len();
    let c = dom::cfg(f);
    // Incoming edges, addressed the way `Term::targets()` orders them.
    let mut in_edges: Vec<Vec<(BlockId, usize)>> = vec![Vec::new(); n];
    for b in 0..n {
        for (i, t) in f.blocks[b].term.targets().iter().enumerate() {
            in_edges[t.block as usize].push((b as BlockId, i));
        }
    }
    // Which blocks re-evaluate when a value descends.
    let mut users: Vec<Vec<BlockId>> = vec![Vec::new(); f.values.len()];
    let mut note = |users: &mut Vec<Vec<BlockId>>, o: Operand, b: BlockId| {
        if let Operand::Val(v) = o {
            let u = &mut users[v as usize];
            if u.last() != Some(&b) {
                u.push(b);
            }
        }
    };
    for b in 0..n {
        let bi = b as BlockId;
        for inst in &f.blocks[b].insts {
            inst.uses(|o| note(&mut users, o, bi));
        }
        f.blocks[b].term.uses(|o| note(&mut users, o, bi));
        // a block argument is consumed by the SUCCESSOR's parameter
        for t in f.blocks[b].term.targets() {
            for a in &t.args {
                note(&mut users, *a, t.block);
            }
        }
    }

    let mut lat = vec![Lat::Top; f.values.len()];
    for (i, v) in f.values.iter().enumerate() {
        if matches!(v.def, Def::FuncParam(_)) {
            lat[i] = Lat::Bot; // an argument is whatever the caller passed
        }
    }
    let mut exec_block = vec![false; n];
    let mut exec_edge: Vec<Vec<bool>> = (0..n)
        .map(|b| vec![false; f.blocks[b].term.targets().len()])
        .collect();
    let mut work: Vec<BlockId> = vec![f.entry];
    let mut queued = vec![false; n];
    queued[f.entry as usize] = true;
    exec_block[f.entry as usize] = true;

    while let Some(b) = work.pop() {
        let bi = b as usize;
        queued[bi] = false;
        if !exec_block[bi] {
            continue;
        }
        let push = |w: &mut Vec<BlockId>, q: &mut Vec<bool>, x: BlockId| {
            if !q[x as usize] {
                q[x as usize] = true;
                w.push(x);
            }
        };
        // (1) block parameters: meet over the executable incoming edges
        for k in 0..f.blocks[bi].params.len() {
            let p = f.blocks[bi].params[k];
            let mut m = Lat::Top;
            for &(pb, ti) in &in_edges[bi] {
                if !exec_edge[pb as usize][ti] {
                    continue;
                }
                let a = f.blocks[pb as usize].term.targets()[ti].args[k];
                m = m.meet(lat_of(&lat, a));
            }
            if m != lat[p as usize] {
                lat[p as usize] = m;
                for &u in &users[p as usize] {
                    push(&mut work, &mut queued, u);
                }
            }
        }
        // (2) instructions
        for i in 0..f.blocks[bi].insts.len() {
            let d = match f.blocks[bi].insts[i].dst() {
                Some(d) => d,
                None => continue,
            };
            let m = transfer(&f.blocks[bi].insts[i], &lat);
            if m != lat[d as usize] {
                lat[d as usize] = m;
                for &u in &users[d as usize] {
                    push(&mut work, &mut queued, u);
                }
            }
        }
        // (3) terminator: which successors become executable
        let live = live_edges(&f.blocks[bi].term, &lat);
        for (ti, on) in live.iter().enumerate() {
            if *on && !exec_edge[bi][ti] {
                exec_edge[bi][ti] = true;
                let t = f.blocks[bi].term.targets()[ti].block;
                exec_block[t as usize] = true;
                push(&mut work, &mut queued, t);
            }
        }
        if let Term::GotoPtr(_, bs) = &f.blocks[bi].term {
            for &t in bs {
                if !exec_block[t as usize] {
                    exec_block[t as usize] = true;
                    push(&mut work, &mut queued, t);
                }
            }
        }
    }

    // ── rewrite ────────────────────────────────────────────────────────────
    let mut map: Vec<Option<Operand>> = vec![None; f.values.len()];
    let mut changed = false;
    for (v, l) in lat.iter().enumerate() {
        if let Lat::Const(o) = l {
            if *o != Operand::Val(v as ValueId) {
                map[v] = Some(*o);
                changed = true;
            }
        }
    }
    if changed {
        rewrite_values(f, &map);
    }
    // Fold each terminator down to the edges the analysis proved executable.
    for b in 0..n {
        if !exec_block[b] || !c.reachable(b as BlockId) {
            continue;
        }
        let live: Vec<bool> = exec_edge[b].clone();
        let nlive = live.iter().filter(|x| **x).count();
        if nlive != 1 || f.blocks[b].term.targets().len() < 2 {
            continue;
        }
        let ti = live.iter().position(|x| *x).unwrap();
        let t = f.blocks[b].term.targets()[ti].clone();
        f.blocks[b].term = Term::Jmp(t);
        changed = true;
    }
    changed
}

fn lat_of(lat: &[Lat], o: Operand) -> Lat {
    match o {
        Operand::Val(v) => lat[v as usize],
        k => Lat::Const(k),
    }
}

/// The transfer function: `fold_inst` applied to the instruction with every
/// known-constant operand substituted. Sharing the folder is what keeps the
/// analysis and the rewrite from ever disagreeing.
fn transfer(inst: &Inst, lat: &[Lat]) -> Lat {
    if inst.effect() != Effect::Pure {
        return Lat::Bot;
    }
    let mut top = false;
    let mut bot = false;
    inst.uses(|o| match lat_of(lat, o) {
        Lat::Top => top = true,
        Lat::Bot => bot = true,
        Lat::Const(_) => {}
    });
    if top && !bot {
        return Lat::Top; // optimistic: no executable definition yet
    }
    let mut sub = inst.clone();
    sub.uses_mut(|o| {
        if let Lat::Const(k) = lat_of(lat, *o) {
            *o = k;
        }
    });
    match fold::fold_inst(&sub) {
        Some(Operand::Val(v)) => lat[v as usize],
        Some(k) => Lat::Const(k),
        None => Lat::Bot,
    }
}

/// Which of `term.targets()` may execute under the current lattice.
fn live_edges(term: &Term, lat: &[Lat]) -> Vec<bool> {
    let n = term.targets().len();
    let all = |on: bool| vec![on; n];
    match term {
        Term::Jmp(_) => all(true),
        Term::Br(c, ..) => match lat_of(lat, *c) {
            Lat::Top => all(false),
            Lat::Const(Operand::Imm(k)) => vec![k != 0, k == 0],
            _ => all(true),
        },
        Term::Switch(c, ty, arms, _) => match lat_of(lat, *c) {
            Lat::Top => all(false),
            Lat::Const(Operand::Imm(k)) => {
                let k = crate::hir::interp::sext(k as u64, *ty);
                let hit = arms.iter().position(|(v, _)| *v == k);
                (0..n).map(|i| Some(i) == hit || (hit.is_none() && i == n - 1)).collect()
            }
            _ => all(true),
        },
        _ => all(true),
    }
}
