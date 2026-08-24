// gvn — dominator-scoped global value numbering (REARCH §4 row 4).
//
// This one pass absorbs four classical ones, which is why the ladder has no
// separate CSE, copy-propagation, constant-folding or reassociation row: an
// expression is replaced by an EARLIER equal expression whenever one dominates
// it, and `fold::fold_inst` supplies the constant and algebraic cases on the way
// past. Simpson's RPO value numbering and Alpern-Wegman-Zadeck partitioning are
// stronger on loop-carried equalities; the dominator-scoped form (Briggs, Cooper
// & Simpson 1997) is the one gcc -O1 ships and needs no extra fixpoint.
//
// Commuting square, in two halves:
//   * REDUNDANCY. If `e₂` has the same opcode, type and operand VALUES as `e₁`
//     and `e₁`'s block dominates `e₂`'s, then on every run reaching `e₂` the
//     instruction `e₁` has already executed and produced that value — the
//     operands are SSA values, so they cannot have changed in between. Only
//     `Effect::Pure` instructions are numbered, so nothing observable moves.
//     A pure instruction that TRAPS (`sdiv` by zero) is safe for the same
//     reason: the dominating copy traps first, so the run is ⊥ either way.
//   * IDENTITY. Every row of `fold::fold_inst` is `⟦L⟧ = ⟦R⟧` — see fold.rs.
use super::*;
use std::collections::HashMap;

/// An operand, flattened into something hashable.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct OpKey(u8, i64);

fn opkey(o: Operand) -> OpKey {
    match o {
        Operand::Val(v) => OpKey(0, v as i64),
        Operand::Imm(k) => OpKey(1, k),
        Operand::Fimm(k) => OpKey(2, k as i64),
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Bin(u8, u8, OpKey, OpKey),
    Un(u8, u8, OpKey),
    Cmp(u8, u8, OpKey, OpKey),
    Cvt(u8, u8, u8, OpKey),
    Slot(u32, i64),
    Sym(super::Sym),
    Sel(u8, OpKey, OpKey, OpKey),
}

fn tyk(t: Ty) -> u8 {
    t as u8
}

/// `a·b = b·a` for these; normalizing the operand order is what makes the two
/// spellings of one expression hash to one key.
fn commutative(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Add
            | BinOp::Mul
            | BinOp::And
            | BinOp::Or
            | BinOp::Xor
            | BinOp::FAdd
            | BinOp::FMul
            | BinOp::SMulHi
            | BinOp::UMulHi
    )
}

fn key_of(inst: &Inst, r: &dyn Fn(Operand) -> Operand) -> Option<Key> {
    Some(match inst {
        Inst::Bin { op, ty, a, b, .. } => {
            let (mut x, mut y) = (opkey(r(*a)), opkey(r(*b)));
            if commutative(*op) && y < x {
                std::mem::swap(&mut x, &mut y);
            }
            Key::Bin(*op as u8, tyk(*ty), x, y)
        }
        Inst::Un { op, ty, a, .. } => Key::Un(*op as u8, tyk(*ty), opkey(r(*a))),
        Inst::Cmp { op, ty, a, b, .. } => {
            let (mut x, mut y) = (opkey(r(*a)), opkey(r(*b)));
            if matches!(op, CmpOp::Eq | CmpOp::Ne | CmpOp::FOeq | CmpOp::FUne | CmpOp::FUno) && y < x
            {
                std::mem::swap(&mut x, &mut y);
            }
            Key::Cmp(*op as u8, tyk(*ty), x, y)
        }
        Inst::Cvt { op, from, to, a, .. } => {
            Key::Cvt(*op as u8, tyk(*from), tyk(*to), opkey(r(*a)))
        }
        Inst::SlotAddr { slot, off, .. } => Key::Slot(*slot, *off),
        Inst::SymAddr { sym, .. } => Key::Sym(sym.clone()),
        Inst::Select { ty, c, a, b, .. } => {
            Key::Sel(tyk(*ty), opkey(r(*c)), opkey(r(*a)), opkey(r(*b)))
        }
        _ => return None,
    })
}

pub fn run(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let mut map: Vec<Option<Operand>> = vec![None; f.values.len()];
    let mut dead: Vec<(usize, usize)> = Vec::new();
    let mut table: HashMap<Key, ValueId> = HashMap::new();
    // One undo entry per insertion, delimited by scope markers, so leaving a
    // dominator subtree restores exactly the table its parent had.
    let mut undo: Vec<Option<Key>> = Vec::new();

    // explicit DFS over the dominator tree (a translation unit nests deeper
    // than the Rust stack tolerates)
    let mut stack: Vec<(BlockId, usize)> = vec![(f.entry, 0)];
    visit(f, f.entry, &mut map, &mut dead, &mut table, &mut undo);
    while let Some(&mut (b, ref mut i)) = stack.last_mut() {
        if *i < dt.kids[b as usize].len() {
            let k = dt.kids[b as usize][*i];
            *i += 1;
            undo.push(None); // scope marker
            visit(f, k, &mut map, &mut dead, &mut table, &mut undo);
            stack.push((k, 0));
        } else {
            stack.pop();
            while let Some(e) = undo.pop() {
                match e {
                    Some(k) => {
                        table.remove(&k);
                    }
                    None => break,
                }
            }
        }
    }

    if dead.is_empty() && map.iter().all(|x| x.is_none()) {
        return false;
    }
    rewrite_values(f, &map);
    dead.sort_unstable();
    for &(b, i) in dead.iter().rev() {
        f.blocks[b].insts.remove(i);
    }
    refresh_defs(f);
    true
}

fn visit(
    f: &Func,
    b: BlockId,
    map: &mut Vec<Option<Operand>>,
    dead: &mut Vec<(usize, usize)>,
    table: &mut HashMap<Key, ValueId>,
    undo: &mut Vec<Option<Key>>,
) {
    let bi = b as usize;
    for (i, inst) in f.blocks[bi].insts.iter().enumerate() {
        let d = match inst.dst() {
            Some(d) => d,
            None => continue,
        };
        if inst.effect() != Effect::Pure {
            continue;
        }
        let r = |o: Operand| resolve(map, o);
        // (1) the identity/constant table
        let mut sub = inst.clone();
        sub.uses_mut(|o| *o = r(*o));
        if let Some(o) = fold::fold_inst(&sub) {
            if o != Operand::Val(d) {
                map[d as usize] = Some(o);
                dead.push((bi, i));
                continue;
            }
        }
        // (2) an equal expression already computed on a dominating path
        let k = match key_of(inst, &r) {
            Some(k) => k,
            None => continue,
        };
        match table.get(&k) {
            Some(&prev) => {
                map[d as usize] = Some(Operand::Val(prev));
                dead.push((bi, i));
            }
            None => {
                table.insert(k.clone(), d);
                undo.push(Some(k));
            }
        }
    }
}

fn resolve(map: &[Option<Operand>], o: Operand) -> Operand {
    let mut cur = o;
    for _ in 0..64 {
        match cur {
            Operand::Val(v) => match map.get(v as usize).and_then(|x| *x) {
                Some(n) if n != cur => cur = n,
                _ => return cur,
            },
            _ => return cur,
        }
    }
    cur
}
