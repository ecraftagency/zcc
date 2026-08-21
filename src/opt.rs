// src/opt.rs — PASS tối ưu trên IR (THEORY.md Phần I §A7).
//
// BẤT BIẾN QUẢN TRỊ (Vu 2026-08-20, thay trần 10k LOC ở fork production): mỗi pass P
// phải BẢO TOÀN NGỮ NGHĨA ⟦·⟧, và điều đó KHÔNG được tin bằng lý luận — phải ĐO cơ
// học bằng `ir::tests::equiv` (commuting-square: ⟦A⟧ ≡ ⟦P(A)⟧ trên battery input) +
// `ir::verify` (well-formed sau pass). Test-gate chạy qua `cargo test opt::` — RẺ hơn
// full-suite nhiều bậc (luật đo-tốc-độ), và mạnh hơn (chứng ở IR→IR_ops, không phải md5 binary).
//
// Mỗi pass là hàm THUẦN IR→IR (mutate tại chỗ), trả số rewrite (đo hội tụ). Backend
// KHÔNG cần biết pass nào đã chạy — nó chỉ đọc IR well-formed.
#![allow(dead_code)] // gỡ khi driver nối pipeline --O1 vào emit_ir

use crate::ast::{TyTab, ULONG};
use crate::ir::{
    canon, eval_bin, eval_cast, inst_def, inst_uses, term_targets, term_uses, Callee, Inst, IrFunc,
    Op, Place, Term, Tmp, Un, Val,
};
use std::collections::{HashMap, HashSet};

// Walker MUTATE mọi toán hạng (Val được ĐỌC) của một lệnh — dùng bởi copy/CSE để
// thay use. KHÔNG đụng temp-đích (def). Đối xứng với ir::inst_uses (bản read-only).
fn each_use_mut(i: &mut Inst, mut g: impl FnMut(&mut Val)) {
    match i {
        Inst::Bin(_, _, _, a, b) => {
            g(a);
            g(b);
        }
        Inst::Un(_, _, _, a) | Inst::Copy(_, _, a) | Inst::Load(_, _, a) | Inst::Cast(_, _, _, a) => {
            g(a)
        }
        Inst::Store(_, a, b) | Inst::Memcpy(a, b, _) => {
            g(a);
            g(b);
        }
        Inst::Zero(a, _)
        | Inst::VaStart(a)
        | Inst::VaArg(_, a, _, _)
        | Inst::GotoPtr(a)
        | Inst::Alloca(_, a) => g(a),
        Inst::Overflow(_, _, _, _, _, a, b, rp) => {
            g(a);
            g(b);
            g(rp);
        }
        Inst::Lea(..)
        | Inst::FunAddr(..)
        | Inst::LabelAddr(..)
        | Inst::VaArea(..) => {}
        Inst::Call(_, c, args, _) => {
            if let Callee::Ptr(p) = c {
                g(p)
            }
            for a in args {
                g(a)
            }
        }
        Inst::CallX(_, c, args, _, _) => {
            if let Callee::Ptr(p) = c {
                g(p)
            }
            for (a, _) in args {
                g(a)
            }
        }
        Inst::Sync(_, _, args, _, _) => {
            for a in args {
                g(a)
            }
        }
        Inst::Asm(_, ops) => {
            for op in ops {
                if let Some(x) = &mut op.inp {
                    g(x)
                }
                if let Some(x) = &mut op.wb {
                    g(x)
                }
            }
        }
    }
}
fn each_use_term_mut(t: &mut Term, mut g: impl FnMut(&mut Val)) {
    match t {
        Term::Br(c, ..) => g(c),
        Term::Ret(Some(r)) => g(r),
        _ => {}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 1 — CONSTANT FOLDING (partial evaluation).
//
// Định lý (rewrite-soundness, THEORY §A7): ⟦Bin(op, Imm a, Imm b)⟧ = ⟦Imm(eval_op(a,b))⟧,
// tương tự Un/Cast hằng. Đúng BY CONSTRUCTION vì fold GỌI CHÍNH `eval_bin/eval_cast/canon`
// mà interp dùng — folder và interpreter là MỘT hàm nghĩa, không thể lệch (faithfulness).
//
// Phạm vi (thận trọng, tránh UB & rounding):
//   • CHỈ integer immediate. Float (FImm) HOÃN: interp mô hình f64, fold f32 có thể
//     lệch rounding so với backend s-reg → giữ nguyên lệnh, để hardware quyết định.
//   • Div/Rem cho 0 → eval_bin trả Err → KHÔNG fold (giữ lệnh, giữ hành vi UB của target).
//   • const-branch: Br(Imm c)→Jmp (mở đường DCE xoá block chết sau này).
// KHÔNG propagate hằng qua temp (đó là copy_prop, pass 3) — pass này chỉ fold toán hạng-hằng-sẵn.
// ─────────────────────────────────────────────────────────────────────────────
pub fn const_fold(tt: &TyTab, f: &mut IrFunc) -> u32 {
    let mut n = 0u32;
    for blk in f.blocks.iter_mut() {
        for inst in blk.insts.iter_mut() {
            let repl: Option<Inst> = match inst {
                Inst::Bin(d, op, ty, Val::Imm(x), Val::Imm(y)) if !tt.is_float(*ty) => {
                    // Err (chia/mod 0) → None: KHÔNG fold UB thành hằng.
                    eval_bin(tt, *op, *ty, *x, *y).ok().map(|r| Inst::Copy(*d, *ty, Val::Imm(r)))
                }
                Inst::Un(d, op, ty, Val::Imm(x)) if !tt.is_float(*ty) => {
                    let r = match op {
                        Un::Neg => canon(tt, *ty, x.wrapping_neg()),
                        Un::BNot => canon(tt, *ty, !*x),
                    };
                    Some(Inst::Copy(*d, *ty, Val::Imm(r)))
                }
                Inst::Cast(d, from, to, Val::Imm(x)) if !tt.is_float(*from) && !tt.is_float(*to) => {
                    Some(Inst::Copy(*d, *to, Val::Imm(eval_cast(tt, *from, *to, *x))))
                }
                _ => None,
            };
            if let Some(r) = repl {
                *inst = r;
                n += 1;
            }
        }
        let newterm: Option<Term> = match &blk.term {
            Term::Br(Val::Imm(c), t, e) => Some(Term::Jmp(if *c != 0 { *t } else { *e })),
            _ => None,
        };
        if let Some(t) = newterm {
            blk.term = t;
            n += 1;
        }
    }
    n
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 2 — DEAD CODE ELIMINATION (liveness).
//
// Định lý (THEORY §A7): một lệnh THUẦN (không side-effect) mà temp-đích KHÔNG được
// đọc ở đâu ⟹ xoá nó bảo toàn ⟦·⟧ (kết quả quan sát không phụ thuộc giá trị chết).
// Thuần = Bin/Un/Copy/Lea/Cast/Load (Load chỉ ĐỌC mem, không ghi → xoá vô hại trong
// mô hình CORE, không volatile). KHÔNG thuần = Call (side-effect), Store/Memcpy (ghi
// mem), Opaque (bảo thủ, chưa biết) → GIỮ dù đích chết.
//
// Liveness dùng ở đây là flow-INSENSITIVE (dù-ở-đâu-cũng-tính-sống): xấp xỉ AN TOÀN
// (giữ NHIỀU hơn cần) — chỉ xoá temp KHÔNG đọc ở BẤT KỲ đâu ⟹ chắc chắn chết. Lặp
// tới fixpoint: xoá lệnh làm toán hạng của nó thành chết → vòng sau xoá tiếp.
// ─────────────────────────────────────────────────────────────────────────────
fn is_pure(i: &Inst) -> bool {
    matches!(
        i,
        Inst::Bin(..) | Inst::Un(..) | Inst::Copy(..) | Inst::Lea(..) | Inst::Cast(..) | Inst::Load(..)
    )
}

pub fn dce(f: &mut IrFunc) -> u32 {
    let mut removed = 0u32;
    let mut buf: Vec<u32> = Vec::new();
    loop {
        // Liveness: temp nào được đọc ở BẤT KỲ inst/term nào?
        let mut used = vec![false; f.temps.len()];
        for b in &f.blocks {
            for i in &b.insts {
                buf.clear();
                inst_uses(i, &mut buf);
                for &u in &buf {
                    used[u as usize] = true;
                }
            }
            buf.clear();
            term_uses(&b.term, &mut buf);
            for &u in &buf {
                used[u as usize] = true;
            }
        }
        let mut any = false;
        for b in f.blocks.iter_mut() {
            let before = b.insts.len();
            b.insts.retain(|i| match inst_def(i) {
                Some(d) if !used[d as usize] && is_pure(i) => false, // chết + thuần → xoá
                _ => true,
            });
            let cut = before - b.insts.len();
            if cut > 0 {
                removed += cut as u32;
                any = true;
            }
        }
        if !any {
            break;
        }
    }
    removed
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 3 — COPY PROPAGATION (Leibniz: thay-bằng-bằng).
//
// Định lý (THEORY §A7): với `t = Copy(src)`, thay mọi USE của t bằng src bảo toàn
// ⟦·⟧ MIỄN LÀ giá trị của src tại điểm dùng = giá trị tại điểm copy. Điều kiện đủ
// AN TOÀN (không cần dominator-tree):
//   • src = Imm/FImm: HẰNG — bất biến ở mọi điểm chương trình ⟹ luôn thay được.
//   • src = Tmp(s) với s SINGLE-DEF: giá trị s bất biến (định nghĩa đúng 1 lần), và
//     copy đọc s ⟹ def(s) đứng trước copy ⟹ trước mọi use của t (lowering có cấu
//     trúc: use-sau-def). ⟹ thay t bằng s an toàn.
// Chỉ propagate temp t SINGLE-DEF (đa-def như `res` của Cond: giá trị phụ thuộc
// đường đi → KHÔNG thay). Giải chuỗi copy (t←u←Imm) về nguồn gốc. KHÔNG xoá lệnh
// Copy (để DCE dọn khi thành chết) — pass này chỉ rewrite USE. equiv gate double-check.
// ─────────────────────────────────────────────────────────────────────────────
fn resolve(subst: &[Option<Val>], v: Val) -> Val {
    let mut cur = v;
    for _ in 0..=subst.len() {
        match cur {
            Val::Tmp(t) => match subst[t as usize] {
                Some(next) if !matches!(next, Val::Tmp(x) if x == t) => cur = next,
                _ => return cur,
            },
            _ => return cur,
        }
    }
    cur
}

pub fn copy_prop(f: &mut IrFunc) -> u32 {
    let nt = f.temps.len();
    let mut defcnt = vec![0u32; nt];
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = inst_def(i) {
                defcnt[d as usize] += 1;
            }
        }
    }
    // Bảng thay: temp single-def bởi Copy(src) với src hằng hoặc single-def-tmp.
    let mut subst: Vec<Option<Val>> = vec![None; nt];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Copy(d, _, src) = i
                && defcnt[*d as usize] == 1 {
                    let ok = match src {
                        Val::Imm(_) | Val::FImm(_) => true,
                        Val::Tmp(s) => defcnt[*s as usize] == 1,
                    };
                    if ok {
                        subst[*d as usize] = Some(*src);
                    }
                }
        }
    }
    let mut n = 0u32;
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            each_use_mut(i, |v| {
                let r = resolve(&subst, *v);
                if !matches!((*v, r), (Val::Tmp(a), Val::Tmp(b)) if a == b)
                    && !matches!(*v, Val::Imm(_) | Val::FImm(_)) {
                        *v = r;
                        n += 1;
                    }
            });
        }
        each_use_term_mut(&mut b.term, |v| {
            let r = resolve(&subst, *v);
            if !matches!((*v, r), (Val::Tmp(a), Val::Tmp(b)) if a == b)
                && !matches!(*v, Val::Imm(_) | Val::FImm(_)) {
                    *v = r;
                    n += 1;
                }
        });
    }
    n
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 4 — COMMON SUBEXPRESSION ELIMINATION (value numbering).
//
// Định lý (THEORY §A7): hai lệnh THUẦN cùng (op, kiểu, toán-hạng) sinh CÙNG giá trị
// ⟹ lệnh sau thay bằng Copy(kết-quả-lệnh-trước). Phạm vi AN TOÀN không cần alias-
// analysis / dominator-tree:
//   • BLOCK-LOCAL (reset cache mỗi block): trong 1 block, temp là single-def (lowering
//     có cấu trúc) ⟹ value-number = chính Val toán hạng, không cần đánh số lại.
//   • Số học Bin/Un/Cast: THUẦN giá trị (không đọc mem) ⟹ cache sống suốt block.
//     Op giao hoán (Add/Mul/And/Or/Xor/Eq/Ne) canonical-hoá thứ tự toán hạng (a+b≡b+a).
//   • Load: value-number theo (địa-chỉ, kiểu), nhưng cache load bị XOÁ SẠCH (bảo thủ)
//     tại BẤT KỲ ghi-mem nào (Store/Memcpy/Call/Opaque) — không giả định non-alias.
//     ⟹ chỉ CSE hai Load cùng địa chỉ khi KHÔNG có ghi-mem xen giữa (available-loads).
// Thay lệnh trùng bằng Copy (DCE/copy-prop dọn sau). equiv gate double-check aliasing.
// ─────────────────────────────────────────────────────────────────────────────
fn enc(v: &Val) -> (u8, i64) {
    match v {
        Val::Tmp(t) => (0, *t as i64),
        Val::Imm(x) => (1, *x),
        Val::FImm(b) => (2, *b as i64),
    }
}
/// Khoá value-number nhị phân: (tag-op, kiểu, toán-hạng-1, toán-hạng-2). Giao hoán → sort.
fn bin_key(op: Op, ty: u32, a: &Val, b: &Val) -> (u16, u32, (u8, i64), (u8, i64)) {
    let commutative = matches!(op, Op::Add | Op::Mul | Op::And | Op::Or | Op::Xor | Op::Eq | Op::Ne);
    let (mut o1, mut o2) = (enc(a), enc(b));
    if commutative && o1 > o2 {
        std::mem::swap(&mut o1, &mut o2);
    }
    (op as u16, ty, o1, o2)
}

pub fn cse(f: &mut IrFunc) -> u32 {
    let mut n = 0u32;
    for b in f.blocks.iter_mut() {
        // arith: khoá (op-tag, ty, o1, o2). Un tag=100+, Cast tag=200 (from ở o2).
        let mut arith: HashMap<(u16, u32, (u8, i64), (u8, i64)), Tmp> = HashMap::new();
        // loads: khoá (địa-chỉ-enc, ty). Xoá sạch tại mọi ghi-mem.
        let mut loads: HashMap<((u8, i64), u32), Tmp> = HashMap::new();
        for i in b.insts.iter_mut() {
            // memory-kill BY-CONSTRUCTION: giữ load-cache CHỈ qua inst chứng-minh-KHÔNG-
            // ghi-mem; mọi thứ khác xoá. Đảo allowlist-writer cũ (correct-by-vigilance:
            // sót Overflow/Zero/VaStart/VaArg — đều ghi mem → dùng lại load cũ = miscompile
            // pr84169). Inst exotic mới mặc định kill → an toàn, không âm thầm giữ load.
            if !matches!(i,
                Inst::Bin(..) | Inst::Un(..) | Inst::Copy(..) | Inst::Load(..) | Inst::Lea(..)
                | Inst::Cast(..) | Inst::FunAddr(..) | Inst::LabelAddr(..) | Inst::VaArea(..)
            ) {
                loads.clear(); // ghi-mem (hoặc không rõ) → memory-kill bảo thủ
            }
            let repl: Option<Inst> = match i {
                Inst::Bin(d, op, ty, a, bb) => {
                    let k = bin_key(*op, *ty, a, bb);
                    match arith.get(&k) {
                        Some(&prev) => Some(Inst::Copy(*d, *ty, Val::Tmp(prev))),
                        None => {
                            arith.insert(k, *d);
                            None
                        }
                    }
                }
                Inst::Un(d, op, ty, a) => {
                    let k = (100u16 + *op as u16, *ty, enc(a), (9, 0));
                    match arith.get(&k) {
                        Some(&prev) => Some(Inst::Copy(*d, *ty, Val::Tmp(prev))),
                        None => {
                            arith.insert(k, *d);
                            None
                        }
                    }
                }
                Inst::Cast(d, from, to, a) => {
                    let k = (200u16, *to, enc(a), (9, *from as i64));
                    match arith.get(&k) {
                        Some(&prev) => Some(Inst::Copy(*d, *to, Val::Tmp(prev))),
                        None => {
                            arith.insert(k, *d);
                            None
                        }
                    }
                }
                Inst::Load(d, ty, addr) => {
                    let k = (enc(addr), *ty);
                    match loads.get(&k) {
                        Some(&prev) => Some(Inst::Copy(*d, *ty, Val::Tmp(prev))),
                        None => {
                            loads.insert(k, *d);
                            None
                        }
                    }
                }
                // Lea(Local off) THUẦN, giá trị = địa chỉ khung (bất biến) → VN được,
                // dedup địa-chỉ để load-CSE khớp qua pipeline (Global/Str bỏ qua).
                Inst::Lea(d, Place::Local(off)) => {
                    let k = (300u16, 0u32, (3, *off as i64), (9, 0));
                    match arith.get(&k) {
                        Some(&prev) => Some(Inst::Copy(*d, ULONG, Val::Tmp(prev))),
                        None => {
                            arith.insert(k, *d);
                            None
                        }
                    }
                }
                _ => None,
            };
            if let Some(r) = repl {
                *i = r;
                n += 1;
            }
        }
    }
    n
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 5 — REGISTER ALLOCATION (graph coloring, Chaitin–Briggs).
//
// NP-complete (THEORY §C2 — tô màu đồ thị) ⟹ dùng HEURISTIC simplify/spill, KHÔNG
// đòi tối ưu tuyệt đối. Nhưng TÍNH ĐÚNG (coloring hợp lệ) verify được trong P.
//
// Correctness ở đây KHÁC 4 pass trên: interp KHÔNG mô hình register, nên không dùng
// ⟦before⟧=⟦after⟧. Bất biến đúng đắn = BISIMULATION-ĐỔI-TÊN (THEORY §A7): chương
// trình gán-register bisimilar chương trình temp ⟺ hai temp SỐNG ĐỒNG THỜI luôn ở
// vị trí KHÁC nhau (không đè giá trị đang sống). Ta chứng cơ học BẤT BIẾN GIAO THOA:
//   ∀ cạnh (u,v) ∈ interference-graph, color[u] ≠ color[v]  (spill = slot riêng, không đè).
//
// Chuỗi định lý: liveness (dataflow monotone, Kleene fixpoint) → interference graph
// (u giao v ⟺ cùng sống tại một def) → coloring (simplify degree<k / spill) → verify.
// ─────────────────────────────────────────────────────────────────────────────

/// Liveness flow-SENSITIVE (backward dataflow, THEORY §B3 fixpoint trên lattice 2^Tmp).
pub struct Liveness {
    pub live_in: Vec<Vec<bool>>,
    pub live_out: Vec<Vec<bool>>,
}

fn successors(f: &IrFunc) -> Vec<Vec<u32>> {
    let mut out = Vec::with_capacity(f.blocks.len());
    let mut buf = Vec::new();
    for b in &f.blocks {
        buf.clear();
        term_targets(&b.term, &mut buf);
        out.push(buf.clone());
    }
    out
}

pub fn liveness(f: &IrFunc) -> Liveness {
    let nb = f.blocks.len();
    let nt = f.temps.len();
    // gen (use trước def trong block) + kill (def trong block)
    let mut useb = vec![vec![false; nt]; nb];
    let mut defb = vec![vec![false; nt]; nb];
    let mut buf = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut defined = vec![false; nt];
        for i in &b.insts {
            buf.clear();
            inst_uses(i, &mut buf);
            for &u in &buf {
                if !defined[u as usize] {
                    useb[bi][u as usize] = true;
                }
            }
            if let Some(d) = inst_def(i) {
                defined[d as usize] = true;
                defb[bi][d as usize] = true;
            }
        }
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            if !defined[u as usize] {
                useb[bi][u as usize] = true;
            }
        }
    }
    let succ = successors(f);
    let mut live_in = vec![vec![false; nt]; nb];
    let mut live_out = vec![vec![false; nt]; nb];
    loop {
        let mut changed = false;
        for bi in (0..nb).rev() {
            let mut lo = vec![false; nt];
            for &s in &succ[bi] {
                for t in 0..nt {
                    if live_in[s as usize][t] {
                        lo[t] = true;
                    }
                }
            }
            let mut li = useb[bi].clone();
            for t in 0..nt {
                if lo[t] && !defb[bi][t] {
                    li[t] = true;
                }
            }
            if lo != live_out[bi] {
                live_out[bi] = lo;
                changed = true;
            }
            if li != live_in[bi] {
                live_in[bi] = li;
                changed = true;
            }
        }
        if !changed {
            break; // fixpoint (Kleene): không set nào lớn lên nữa
        }
    }
    Liveness { live_in, live_out }
}

/// Đồ thị giao thoa: u—v ⟺ u,v cùng sống tại một điểm định nghĩa (không thể chung register).
pub fn interference(f: &IrFunc, lv: &Liveness) -> Vec<HashSet<Tmp>> {
    let nt = f.temps.len();
    let mut adj: Vec<HashSet<Tmp>> = vec![HashSet::new(); nt];
    let mut buf = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut live = lv.live_out[bi].clone();
        // toán hạng của terminator sống ở đuôi block
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            live[u as usize] = true;
        }
        for i in b.insts.iter().rev() {
            if let Some(d) = inst_def(i) {
                for (t, &alive) in live.iter().enumerate() {
                    if alive && t as u32 != d {
                        adj[d as usize].insert(t as u32);
                        adj[t].insert(d);
                    }
                }
                live[d as usize] = false;
            }
            buf.clear();
            inst_uses(i, &mut buf);
            for &u in &buf {
                live[u as usize] = true;
            }
        }
    }
    adj
}

/// Kết quả tô màu: color[t]=Some(r) → register r; None → spill (slot stack riêng).
pub struct Alloc {
    pub color: Vec<Option<u32>>,
    pub spilled: Vec<Tmp>,
    pub k: u32,
}

pub fn color(adj: &[HashSet<Tmp>], k: u32) -> Alloc {
    let nt = adj.len();
    let mut removed = vec![false; nt];
    let mut degree: Vec<usize> = adj.iter().map(|s| s.len()).collect();
    let mut stack: Vec<Tmp> = Vec::new();
    // SIMPLIFY: đẩy node degree<k lên stack (chắc tô được); hết → chọn max-degree làm
    // potential-spill (Briggs: có thể vẫn tô được ở select).
    for _ in 0..nt {
        let low = (0..nt).find(|&v| !removed[v] && degree[v] < k as usize);
        let v = match low {
            Some(v) => v,
            None => match (0..nt).filter(|&v| !removed[v]).max_by_key(|&v| degree[v]) {
                Some(v) => v,
                None => break,
            },
        };
        removed[v] = true;
        stack.push(v as u32);
        for &nb in &adj[v] {
            if !removed[nb as usize] {
                degree[nb as usize] -= 1;
            }
        }
    }
    // SELECT: pop, gán màu nhỏ nhất khác hàng xóm; hết màu → spill thật (None).
    let mut colr = vec![None; nt];
    let mut spilled = Vec::new();
    while let Some(v) = stack.pop() {
        let mut used = vec![false; k as usize];
        for &nb in &adj[v as usize] {
            if let Some(c) = colr[nb as usize]
                && (c as usize) < k as usize {
                    used[c as usize] = true;
                }
        }
        match (0..k).find(|&c| !used[c as usize]) {
            Some(c) => colr[v as usize] = Some(c),
            None => spilled.push(v),
        }
    }
    Alloc { color: colr, spilled, k }
}

/// CHỨNG cơ học bất biến giao thoa: cạnh nào cũng khác màu. Đây là "P-verify" của
/// lời-giải NP — regalloc có thể heuristic, nhưng tính đúng thì kiểm được rẻ.
pub fn verify_coloring(adj: &[HashSet<Tmp>], al: &Alloc) -> Result<(), String> {
    for u in 0..adj.len() {
        if let Some(cu) = al.color[u] {
            for &v in &adj[u] {
                if al.color[v as usize] == Some(cu) {
                    return Err(format!("giao thoa (t{u},t{v}) cùng register {cu}"));
                }
            }
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// ORCHESTRATOR — pipeline IR→IR (passes 1-4) tới FIXPOINT. Mỗi pass bảo toàn ⟦·⟧
// (đã chứng riêng), nên hợp thành cũng bảo toàn ⟦·⟧ (đóng dưới phép hợp). Lặp vì pass
// này mở cơ hội cho pass kia (copy-prop→const-fold gấp hằng; CSE→copy-prop→DCE dọn).
// Chốt vòng lặp bằng "không rewrite nào nữa" (hội tụ) + trần cứng chống loạn.
// (Regalloc KHÔNG ở đây: nó sinh assignment cho BACKEND tiêu thụ, không phải IR→IR;
//  backend hiện spill-per-node chưa dùng — sẽ nối khi flip default sang IR, Bước B.)
// ─────────────────────────────────────────────────────────────────────────────
pub fn optimize(tt: &TyTab, f: &mut IrFunc) {
    for _ in 0..32 {
        let mut n = 0;
        n += const_fold(tt, f);
        n += copy_prop(f);
        n += cse(f);
        n += dce(f);
        if n == 0 {
            break; // fixpoint: không lệnh nào đổi nữa
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{TyTab, INT};
    use crate::ir::tests::{compile, equiv, interp, mk};
    use crate::ir::{verify, Block};

    fn count_calls(f: &IrFunc) -> usize {
        f.blocks.iter().flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::Call(..))).count()
    }
    fn count_loads(ir: &[IrFunc]) -> usize {
        ir.iter().flat_map(|f| &f.blocks).flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::Load(..))).count()
    }

    // Pass 1 fold toán hạng-hằng, equiv giữ, kết quả = Copy(Imm đã tính).
    #[test]
    fn cf_folds_const_bin() {
        let tt = TyTab::new();
        let before = vec![mk(
            "f",
            vec![INT],
            vec![],
            0,
            INT,
            vec![Block {
                insts: vec![Inst::Bin(0, Op::Mul, INT, Val::Imm(6), Val::Imm(7))],
                term: Term::Ret(Some(Val::Tmp(0))),
            }],
        )];
        let mut after = before.clone();
        let n = const_fold(&tt, &mut after[0]);
        assert!(n >= 1, "phải fold ≥1");
        verify(&after[0]).unwrap();
        equiv(&tt, &before, &after, "f").expect("const-fold phải bảo toàn ⟦·⟧");
        assert!(matches!(after[0].blocks[0].insts[0], Inst::Copy(0, _, Val::Imm(42))));
        assert_eq!(interp(&tt, &after, "f", &[]).unwrap(), 42);
    }

    // const-branch: Br(Imm 0) → Jmp(else); interp đi đúng nhánh.
    #[test]
    fn cf_const_branch() {
        let tt = TyTab::new();
        let before = vec![mk(
            "h",
            vec![],
            vec![],
            0,
            INT,
            vec![
                Block { insts: vec![], term: Term::Br(Val::Imm(0), 1, 2) },
                Block { insts: vec![], term: Term::Ret(Some(Val::Imm(10))) },
                Block { insts: vec![], term: Term::Ret(Some(Val::Imm(20))) },
            ],
        )];
        let mut after = before.clone();
        let n = const_fold(&tt, &mut after[0]);
        assert!(n >= 1);
        assert!(matches!(after[0].blocks[0].term, Term::Jmp(2)));
        verify(&after[0]).unwrap();
        equiv(&tt, &before, &after, "h").expect("const-branch phải bảo toàn ⟦·⟧");
        assert_eq!(interp(&tt, &after, "h", &[]).unwrap(), 20);
    }

    // Trên code THẬT (parser đã fold sẵn hằng nguồn ⟹ pass phần lớn no-op) — bất
    // biến then chốt: dù làm ít hay nhiều, const-fold KHÔNG BAO GIỜ đổi ⟦·⟧.
    #[test]
    fn cf_preserves_real() {
        for (nm, src, entry) in [
            ("a", "int g(int a,int b){int c=a+b;if(a>b)return c*2;return c-1;}", "g"),
            ("b", "int s(int n){int t=0;int i;for(i=0;i<n;i=i+1)t=t+i*3;return t;}", "s"),
            ("c", "int m(int a,int b){return a%b + a/b;}", "m"),
        ] {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                const_fold(&ast.tt, f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
            }
            equiv(&ast.tt, &ir, &opt, entry).unwrap_or_else(|e| panic!("{nm}: {e}"));
        }
    }

    // DCE xoá biểu-thức-lệnh chết (`a*b;`) + toán hạng của nó, equiv giữ nguyên.
    #[test]
    fn dce_removes_dead() {
        let (ast, ir) = compile("dce1", "int g(int a,int b){a*b; return a+b;}");
        let mut opt = ir.clone();
        let removed: u32 = opt.iter_mut().map(dce).sum();
        assert!(removed >= 1, "phải xoá ≥1 lệnh chết");
        for f in &opt {
            verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
        }
        equiv(&ast.tt, &ir, &opt, "g").expect("DCE phải bảo toàn ⟦·⟧");
    }

    // DCE KHÔNG được xoá Call dù kết quả không dùng (side-effect). Chứng cấu trúc:
    // số Call giữ nguyên; + equiv.
    #[test]
    fn dce_keeps_call() {
        let (ast, ir) = compile("dce2", "int side(int x){return x;} int k(int a){side(a); return a+1;}");
        let calls_before: usize = ir.iter().map(count_calls).sum();
        let mut opt = ir.clone();
        for f in opt.iter_mut() {
            dce(f);
        }
        let calls_after: usize = opt.iter().map(count_calls).sum();
        assert_eq!(calls_before, calls_after, "DCE KHÔNG được xoá Call");
        assert!(calls_after >= 1);
        for f in &opt {
            verify(f).unwrap();
        }
        equiv(&ast.tt, &ir, &opt, "k").expect("DCE (giữ call) phải bảo toàn ⟦·⟧");
        assert_eq!(interp(&ast.tt, &opt, "k", &[41]).unwrap(), 42);
    }

    #[test]
    fn dce_preserves_real() {
        for (nm, src, entry) in [
            ("a", "int g(int a,int b){int c=a+b;a-b;if(a>b)return c*2;return c-1;}", "g"),
            ("b", "int s(int n){int t=0;int i;for(i=0;i<n;i=i+1){i*i;t=t+i;}return t;}", "s"),
        ] {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                dce(f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
            }
            equiv(&ast.tt, &ir, &opt, entry).unwrap_or_else(|e| panic!("{nm}: {e}"));
        }
    }

    // Copy-prop + const-fold cascade: t0=Copy(5); t1=t0+3 → (prop) t1=5+3 → (fold) 8.
    #[test]
    fn cp_const_cascade() {
        let tt = TyTab::new();
        let before = vec![mk(
            "f",
            vec![INT, INT],
            vec![],
            0,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Copy(0, INT, Val::Imm(5)),
                    Inst::Bin(1, Op::Add, INT, Val::Tmp(0), Val::Imm(3)),
                ],
                term: Term::Ret(Some(Val::Tmp(1))),
            }],
        )];
        let mut opt = before.clone();
        let n = copy_prop(&mut opt[0]);
        assert!(n >= 1, "phải propagate ≥1 use");
        verify(&opt[0]).unwrap();
        equiv(&tt, &before, &opt, "f").expect("copy-prop bảo toàn ⟦·⟧");
        // giờ Bin có toán hạng hằng → const-fold gấp thành 8
        const_fold(&tt, &mut opt[0]);
        equiv(&tt, &before, &opt, "f").expect("cascade bảo toàn ⟦·⟧");
        assert_eq!(interp(&tt, &opt, "f", &[0, 0]).unwrap(), 8);
    }

    #[test]
    fn cp_preserves_real() {
        for (nm, src, entry) in [
            ("a", "int g(int a,int b){int x=a;int y=x+b;return y*x;}", "g"),
            // Cond ⟹ temp `res` ĐA-ĐỊNH-NGHĨA: copy-prop KHÔNG được thay nó.
            ("b", "int c(int a){int r=a>0?1:2;return r+a;}", "c"),
            ("d", "int t(int a,int b){int p=a+b;int q=p;return p*q;}", "t"),
        ] {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                copy_prop(f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
            }
            equiv(&ast.tt, &ir, &opt, entry).unwrap_or_else(|e| panic!("{nm}: {e}"));
        }
    }

    // CSE số học: hai Bin(Add,t0,t1) giống hệt → lệnh sau thành Copy(t2). interp giữ.
    #[test]
    fn cse_arith() {
        let tt = TyTab::new();
        let before = vec![mk(
            "f",
            vec![INT, INT, INT, INT, INT],
            vec![],
            0,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Copy(0, INT, Val::Imm(4)),
                    Inst::Copy(1, INT, Val::Imm(5)),
                    Inst::Bin(2, Op::Add, INT, Val::Tmp(0), Val::Tmp(1)),
                    Inst::Bin(3, Op::Add, INT, Val::Tmp(1), Val::Tmp(0)), // giao hoán → cùng khoá
                    Inst::Bin(4, Op::Mul, INT, Val::Tmp(2), Val::Tmp(3)),
                ],
                term: Term::Ret(Some(Val::Tmp(4))),
            }],
        )];
        let mut after = before.clone();
        let n = cse(&mut after[0]);
        assert!(n >= 1, "phải CSE ≥1");
        assert!(matches!(after[0].blocks[0].insts[3], Inst::Copy(3, _, Val::Tmp(2))));
        verify(&after[0]).unwrap();
        equiv(&tt, &before, &after, "f").expect("CSE bảo toàn ⟦·⟧");
        assert_eq!(interp(&tt, &after, "f", &[]).unwrap(), 81);
    }

    // Load-CSE qua pipeline (cse;copy_prop)²;dce: `s+s` load s HAI lần liền nhau,
    // không ghi-mem xen giữa → gộp còn 1 load. equiv giữ, số Load GIẢM.
    #[test]
    fn cse_load_pipeline() {
        let (ast, ir) = compile("csel", "int f(int a){int s=a*a; return s+s;}");
        let loads0 = count_loads(&ir);
        let mut opt = ir.clone();
        for f in opt.iter_mut() {
            cse(f);
            copy_prop(f);
            cse(f);
            copy_prop(f);
            dce(f);
        }
        for f in &opt {
            verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
        }
        equiv(&ast.tt, &ir, &opt, "f").expect("load-CSE bảo toàn ⟦·⟧");
        assert!(count_loads(&opt) < loads0, "load-CSE phải giảm số Load ({loads0}→{})", count_loads(&opt));
    }

    // An toàn ALIASING: đọc p, GHI p, đọc p lại — load-CSE KHÔNG được gộp qua store.
    // equiv là trọng tài cơ học (nếu gộp sai, giá trị lệch → bắt).
    #[test]
    fn cse_preserves_real() {
        for (nm, src, entry) in [
            ("a", "int f(int a){int p=a;int q=p;p=p+1;return p+q;}", "f"),
            ("b", "int g(int a,int b){return (a+b)*(a+b)+(a+b);}", "g"),
            ("c", "int h(int n){int t=0;int i;for(i=0;i<n;i=i+1)t=t+(i*i)+(i*i);return t;}", "h"),
        ] {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                cse(f);
                copy_prop(f);
                cse(f);
                dce(f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
            }
            equiv(&ast.tt, &ir, &opt, entry).unwrap_or_else(|e| panic!("{nm}: {e}"));
        }
    }

    // Hai temp cùng sống tại một def PHẢI giao thoa (cạnh trong đồ thị).
    fn two_live() -> IrFunc {
        mk(
            "f",
            vec![INT, INT, INT],
            vec![],
            0,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Copy(0, INT, Val::Imm(1)),
                    Inst::Copy(1, INT, Val::Imm(2)),
                    Inst::Bin(2, Op::Add, INT, Val::Tmp(0), Val::Tmp(1)), // t0,t1 cùng sống
                ],
                term: Term::Ret(Some(Val::Tmp(2))),
            }],
        )
    }

    #[test]
    fn interference_known() {
        let f = two_live();
        let adj = interference(&f, &liveness(&f));
        assert!(adj[0].contains(&1) && adj[1].contains(&0), "t0,t1 phải giao thoa");
        // t2 không cùng sống với t0/t1 (chúng chết khi t2 sinh) → không giao thoa
        assert!(!adj[2].contains(&0) && !adj[2].contains(&1));
    }

    // Coloring hợp lệ trên code THẬT (K=8 dư): bất biến giao thoa giữ.
    #[test]
    fn reg_alloc_valid() {
        for (nm, src) in [
            ("a", "int g(int a,int b){int c=a+b;int d=c*a;int e=d-b;return c+d+e;}"),
            ("b", "int s(int n){int t=0;int i;for(i=0;i<n;i=i+1)t=t+i*i;return t;}"),
            ("c", "int m(int a,int b,int c){int x=a*b;int y=b*c;int z=a*c;return x+y+z;}"),
        ] {
            let (_ast, ir) = compile(nm, src);
            for f in &ir {
                let adj = interference(f, &liveness(f));
                let al = color(&adj, 8);
                verify_coloring(&adj, &al).unwrap_or_else(|e| panic!("{nm}/{}: {e}", f.name));
            }
        }
    }

    // K=1 (một register) + hai temp giao thoa ⟹ BẮT BUỘC spill; coloring vẫn HỢP LỆ
    // (spill = slot riêng, không tính vào bất biến register).
    #[test]
    fn reg_alloc_spill() {
        let f = two_live();
        let adj = interference(&f, &liveness(&f));
        let al = color(&adj, 1);
        assert!(!al.spilled.is_empty(), "K=1 phải ép spill ≥1 temp");
        verify_coloring(&adj, &al).expect("coloring (có spill) vẫn phải hợp lệ");
    }

    fn count_insts(ir: &[IrFunc]) -> usize {
        ir.iter().flat_map(|f| &f.blocks).map(|b| b.insts.len()).sum()
    }

    // ORCHESTRATOR: pipeline hội tụ, BẢO TOÀN ⟦·⟧ trên corpus rộng (gồm loop, cond,
    // con trỏ, struct, đệ quy). Đây là ir-gate của toàn pipeline (thay full-suite lúc dev).
    #[test]
    fn optimize_preserves_corpus() {
        let cases: &[(&str, &str, &str, &[i64])] = &[
            ("arith", "int f(int a,int b){return (a+b)*(a+b)-a*b+7;}", "f", &[6, 7]),
            ("cond", "int f(int a){int r=a>0?a*2:-a;return r+1;}", "f", &[5]),
            ("loop", "int f(int n){int s=0;int i;for(i=0;i<n;i=i+1)s=s+i*i;return s;}", "f", &[6]),
            ("ptr", "int f(int x){int y=x;int*p=&y;*p=*p+3;return y*y;}", "f", &[4]),
            ("while", "int f(int n){int a=0,b=1,i=0;while(i<n){int t=a+b;a=b;b=t;i=i+1;}return a;}", "f", &[10]),
            ("rec", "int f(int n){if(n<=1)return 1;return n*f(n-1);}", "f", &[6]),
            ("cse", "int f(int a,int b){int s=a+b;return s*s+s;}", "f", &[3, 4]),
            ("cfold", "int f(int a){int k=2*3+4;return a*k;}", "f", &[5]),
        ];
        for &(nm, src, entry, args) in cases {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                optimize(&ast.tt, f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {nm}/{}: {e}", f.name));
            }
            equiv(&ast.tt, &ir, &opt, entry).unwrap_or_else(|e| panic!("{nm}: {e}"));
            // pipeline phải cho CÙNG kết quả interp như trước (sanity)
            let r0 = interp(&ast.tt, &ir, entry, args).unwrap();
            let r1 = interp(&ast.tt, &opt, entry, args).unwrap();
            assert_eq!(r0, r1, "{nm}: interp lệch sau optimize");
        }
    }

    // Pipeline THỰC SỰ tối ưu (không phải no-op): cfold + CSE giảm số lệnh.
    #[test]
    fn optimize_reduces() {
        let (ast, ir) = compile("red", "int f(int a,int b){int s=a+b;int t=a+b;return s*t+s*t;}");
        let before = count_insts(&ir);
        let mut opt = ir.clone();
        for f in opt.iter_mut() {
            optimize(&ast.tt, f);
        }
        equiv(&ast.tt, &ir, &opt, "f").unwrap();
        assert!(count_insts(&opt) < before, "pipeline phải giảm lệnh ({before}→{})", count_insts(&opt));
    }
}
