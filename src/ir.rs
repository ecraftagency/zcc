// src/ir.rs — BOUNDARY MỚI frontend↔backend (contract IR).
//
// Vị trí:  parser → AST (ast.rs) ──lower──▶ IR (đây) ──▶ codegen/<target> → .s
//
// Bất biến: backend chỉ ĐỌC IR + TyTab, KHÔNG đọc AST/parser. IR là "văn phạm
// trung gian" đã hạ hết ngữ nghĩa nguồn C (lvalue, scope, declarator) — mỗi Inst
// là một hạng 3-address CÓ KIỂU; backend chỉ còn instruction-selection + ABI.
//
// Định lý nền (mỗi type dưới đây map một khái niệm):
//   - Val    = toán hạng của đại số 3-address (biến t hoặc hằng).
//   - Op     = KÝ HIỆU phép toán thuần; NGỮ NGHĨA (ring ℤ/2^n, field ℝ, dấu) do
//              TypeId đi kèm quyết định — tách "phép" khỏi "cấu trúc đại số".
//   - Block+Term = CFG tường minh: mỗi block kết đúng 1 terminator (đảm bảo bằng
//              KIỂU — `term` là field, không phải phần tử cuối vector).
//   - IrFunc = đồ thị điều khiển hữu hạn + bảng kiểu temp (Γ: Tmp → TypeId).
//
// Baseline (Vu chốt 2026-08-20): CHỈ IR, KHOAN optimization. Pass layer để tương
// lai — xem IR.md §5. interp/verifier là proof-checker (test-side, không tính
// trần 10k); verifier ở đây vì nó nhẹ và là automaton kiểm ngay sau lowering.
#![allow(dead_code)] // gỡ khi lowering (step 2) + backend IR→asm (step 3) tiêu thụ

use crate::ast::{Ast, Node, NodeId, SyncOp, Ty, TyTab, TypeId, INT, ULONG, VOID};
use std::collections::HashMap;

pub type Tmp = u32; // định danh temp; đánh index vào IrFunc.temps (bảng kiểu Γ)
pub type BlockId = u32; // đánh index vào IrFunc.blocks; blocks[0] = entry

/// Toán hạng 3-address. KHÔNG lồng biểu thức — parser đã hạ cây thành chuỗi gán.
#[derive(Clone, Copy, Debug)]
pub enum Val {
    Tmp(Tmp),  // giá trị của một temp (SSA-free: temp có thể bị gán lại)
    Imm(i64),  // hằng nguyên (kể cả con trỏ hằng, char, enum) — bề rộng theo ngữ cảnh
    FImm(u64), // hằng dấu phẩy động dưới dạng BIT PATTERN (f32 ở 32 bit thấp / f64)
}

/// Ký hiệu phép toán nhị phân — THUẦN đại số. Dấu (signed/unsigned) và tính
/// float lấy từ TypeId đi kèm Inst::Bin, không mã hoá ở đây. So sánh → {0,1}.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Add, Sub, Mul, Div, Rem, // số học: ℤ/2^n (int) hoặc ℝ (float); Div/Rem xét dấu
    And, Or, Xor, Shl, Shr,  // bit: shr xét dấu (arith vs logic)
    Eq, Ne, Lt, Le, Gt, Ge,  // quan hệ → 0/1; Lt..Ge xét dấu
}

/// Phép toán một ngôi.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Un {
    Neg,  // đối số học (int wrap 2^n / float)
    BNot, // ~ (bù bit)
}
// Ghi chú: `!` logic parser đã hạ thành `== 0` (Op::Eq với Imm 0) → không cần Un.

/// Nơi tính ĐỊA CHỈ tĩnh (Lea). Địa chỉ động (con trỏ + offset) = Bin(Add) trên
/// giá trị con trỏ, KHÔNG có Inst riêng cho Member/Index (đã fold thành Add off).
#[derive(Clone, Debug)]
pub enum Place {
    Local(u32),          // &biến local: offset khung fp-relative (tra frame)
    Global(String, i64), // &global ± offset byte (symbol + hằng)
    Str(u32),            // &string literal thứ i (index vào bảng strs)
}

/// Đích của lời gọi.
#[derive(Clone, Debug)]
pub enum Callee {
    Sym(String), // gọi trực tiếp theo tên hàm
    Ptr(Val),    // gọi gián tiếp qua con trỏ hàm
}

/// Lệnh IR. Hai HẠNG (xem IR.md §2b):
///   CORE   — interp evaluate được, verifier phủ, (tương lai) pass được đụng.
///   OPAQUE — bọc construct exotic (va/atomic/asm/…), hạ 1-1 xuống backend Y NHƯ
///            AST→asm cũ; interp coi hàm chứa nó là "không thuần" (bỏ qua fold).
#[derive(Clone, Debug)]
pub enum Inst {
    // ---- CORE ----
    Bin(Tmp, Op, TypeId, Val, Val), // dst = a ⟨op⟩ b, diễn giải trong TypeId
    Un(Tmp, Un, TypeId, Val),       // dst = ⟨op⟩ a
    Copy(Tmp, TypeId, Val),         // dst = a (đổi tên/nạp hằng)
    Load(Tmp, TypeId, Val),         // dst = *(addr), bề rộng = size(TypeId)
    Store(TypeId, Val, Val),        // *(addr) = val; (ty, addr, val)
    Memcpy(Val, Val, u32),          // copy `size` byte *(src) → *(dst); (dst, src, size).
    // Gán struct/union (C99 6.5.16). CORE, target-independent: interp thực thi được,
    // mỗi backend tự hạ (loop / rep-movs / memcpy libcall) — KHÔNG cần biết AST.
    Lea(Tmp, Place),                // dst = địa chỉ của Place
    Cast(Tmp, TypeId, TypeId, Val), // dst:to = cast(from → to) a  (trunc/ext/f↔i)
    Call(Option<Tmp>, Callee, Vec<Val>, u32), // dst?, callee, args, nfix (ABI variadic)
    // ---- EXOTIC typed (thay dần Opaque; operand-free trước) ----
    // dst = &hàm (GOT nếu extern, adrp/add nếu static). Địa chỉ hằng-symbol,
    // KHÔNG toán hạng Val. Giữ impure (như Opaque) → không DCE/CSE → asm bất biến.
    FunAddr(Tmp, String),   // dst = địa chỉ hàm `name`
    LabelAddr(Tmp, String), // EXT(gcc): dst = &&label (computed-goto) trong hàm hiện tại
    // memset(addr, 0, sz): zero-init struct/array (C99 6.7.8). Void, side-effect
    // ghi bộ nhớ → impure như Store, KHÔNG dst.
    Zero(Val, u32), // *(addr .. addr+sz) = 0
    // Variadic AAPCS64 (C99 7.15). Operand = &va_list. VaStart void; VaArg trả 1 giá
    // trị (struct = địa chỉ). Impure (đọc/ghi va_list + save-area).
    VaStart(Val),                    // khởi tạo *(&ap) từ trạng thái prologue
    VaArg(Tmp, Val, TypeId, u32),    // dst = va_arg(*(&ap), t); tmp = scratch-local (HFA gather)
    // EXT(gcc) __builtin_*_overflow: dst = (a op b tràn?); ghi kết quả vào *(rp).
    // op = mã u8; ta/tb = kiểu toán hạng (dấu), rt = kiểu *(rp) (dấu+rộng). Impure.
    Overflow(Tmp, u8, TypeId, TypeId, TypeId, Val, Val, Val), // dst,op,ta,tb,rt,a,b,rp
    VaArea(Tmp, u32), // builtin __va_area__: dst = x29 + off (đầu vùng arg vô danh)
    // EXT(gcc): computed-goto "goto *e": br qua giá trị. Kết thúc khối theo runtime (block
    // IR sau nó là dead — như Opaque cũ). Impure, KHÔNG dst.
    GotoPtr(Val),
    // C99 6.7.5.2 VLA / __builtin_alloca: dst = con trỏ tới `size` byte cấp trên
    // stack (sub sp, làm tròn 16). Impure (đổi sp) → không DCE/CSE dù dst chết;
    // epilogue `mov sp,x29` thu hồi, reset_sp_base tại label depth-0 (goto-lùi).
    Alloca(Tmp, Val), // dst = &vùng cấp; operand = số byte
    // Call ABI-đầy-đủ (composite arg/ret, tràn reg, float≠8B, long double) — automaton
    // AAPCS64 C.1–C.11. Operand mang KIỂU từng arg (Val = giá trị scalar / ĐỊA CHỈ
    // struct, khớp lower_expr). ret = kiểu trả; sret = local slot khi ret là struct
    // (dst = &slot). Scalar-thuần vẫn đi Inst::Call (nhanh). Impure.
    CallX(Option<Tmp>, Callee, Vec<(Val, TypeId)>, TypeId, u32), // dst?, callee, (val,ty)*, ret, sret-off
    // EXT(gcc) atomics __sync_* (C11 mượn): LL/SC ldaxr/stlxr. Operand = (ptr[, val
    // [, val2]]) → x0/x1/x2. sz = 4|8 (width), ret = kiểu kết quả (dấu, canon x0).
    // dst None ⟺ void (Release/Barrier). Impure (ghi mem + hàng rào).
    Sync(Option<Tmp>, SyncOp, Vec<Val>, u32, TypeId), // dst?, op, operands, size, ret
    // EXT(gcc) inline asm: template + operand đã materialize thành Val (inp = giá trị
    // input / địa chỉ mem nạp vào reg; wb = địa chỉ writeback cho output non-mem).
    // Void, impure (ghi mem qua output/mem-operand + có thể clobber). KHÔNG dst.
    Asm(String, Vec<AsmIrOp>),
}

/// Operand inline-asm đã hạ về IR: metadata ràng buộc (giữ nguyên từ AsmOp) + kiểu
/// + Val đã tính. inp = giá trị input / địa chỉ (mem) nạp vào reg phase-1 (None = pure
/// output). wb = địa chỉ ghi ngược cho output non-mem (None = không ghi ngược).
#[derive(Clone, Debug)]
pub struct AsmIrOp {
    pub out: bool,
    pub rw: bool,
    pub mem: bool,
    pub fp: bool,
    pub tied: Option<u8>,
    pub pin: Option<u8>,
    pub ty: TypeId,
    pub inp: Option<Val>,
    pub wb: Option<Val>,
}

/// Terminator — chuyển điều khiển rời block. Automaton hữu hạn trên tập BlockId.
#[derive(Clone, Debug)]
pub enum Term {
    Jmp(BlockId),                          // nhảy vô điều kiện
    Br(Val, BlockId, BlockId),             // cond ≠ 0 → then, ngược lại → els
    Ret(Option<Val>),                      // trả (void nếu None)
    Unreachable,                           // sau noreturn / chốt fallthrough không tới
}

/// Một khối cơ bản: chuỗi lệnh thẳng + đúng một terminator (bất biến bằng kiểu).
#[derive(Clone, Debug)]
pub struct Block {
    pub insts: Vec<Inst>,
    pub term: Term,
}

/// Hàm ở dạng IR: CFG (blocks) + bảng kiểu temp (Γ) + khung stack.
#[derive(Clone)]
pub struct IrFunc {
    pub name: String,
    pub temps: Vec<TypeId>,        // Γ: temp i có kiểu temps[i]
    pub params: Vec<(u32, TypeId)>, // (offset khung, kiểu) — param vào slot khung
    // theo ABI (backend emit_params); interp seed thẳng vào mem. KHÔNG param-temp.
    pub blocks: Vec<Block>, // blocks[0] = entry
    pub frame: u32,         // kích thước khung (đã tròn 16) — cho Lea(Local)
    pub ret: TypeId,        // kiểu trả
    // EXT(gcc): nhãn C (goto/&&label) → block. Backend phát thêm `lg_fname.name:`
    // tại block để computed-goto (LabelAddr/GotoPtr) resolve được địa chỉ nhãn.
    pub labels: Vec<(String, BlockId)>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Verifier — automaton well-formedness (IR.md §3a). Chạy sau lowering / mỗi pass;
// reject IR hỏng NGAY thay vì để trôi xuống asm rác. v1 (baseline) kiểm:
//   (V1) ref-integrity: mọi Tmp id < |temps|, mọi BlockId đích < |blocks|.
//   (V2) def-coverage : mọi temp được DÙNG phải được ĐỊNH NGHĨA ≥1 lần trong hàm.
//   (V3) entry        : hàm có ≥1 block (blocks[0] = entry).
// (Def-before-use TỪNG-ĐƯỜNG (dominance) là định lý mạnh hơn — mở khi có pass
//  di chuyển lệnh; v1 chỉ cần bảo toàn tính hợp lệ tham chiếu sau lowering thẳng.)
// ─────────────────────────────────────────────────────────────────────────────

/// Temp được ĐỊNH NGHĨA bởi lệnh này (đích), nếu có.
pub(crate) fn inst_def(i: &Inst) -> Option<Tmp> {
    match i {
        Inst::Bin(d, ..)
        | Inst::Un(d, ..)
        | Inst::Copy(d, ..)
        | Inst::Load(d, ..)
        | Inst::Lea(d, ..)
        | Inst::Cast(d, ..)
        | Inst::FunAddr(d, ..)
        | Inst::LabelAddr(d, ..)
        | Inst::VaArg(d, ..)
        | Inst::Overflow(d, ..)
        | Inst::VaArea(d, ..)
        | Inst::Alloca(d, ..) => Some(*d),
        Inst::Call(d, ..) | Inst::CallX(d, ..) | Inst::Sync(d, ..) => *d,
        Inst::Store(..)
        | Inst::Memcpy(..)
        | Inst::Zero(..)
        | Inst::VaStart(..)
        | Inst::GotoPtr(..)
        | Inst::Asm(..) => None,
    }
}

/// Gom mọi Tmp mà lệnh này DÙNG (đọc) vào `out`.
pub(crate) fn inst_uses(i: &Inst, out: &mut Vec<Tmp>) {
    let mut v = |x: &Val| {
        if let Val::Tmp(t) = x {
            out.push(*t)
        }
    };
    match i {
        Inst::Bin(_, _, _, a, b) => {
            v(a);
            v(b);
        }
        Inst::Un(_, _, _, a) | Inst::Copy(_, _, a) | Inst::Load(_, _, a) | Inst::Cast(_, _, _, a) => {
            v(a)
        }
        Inst::Store(_, a, b) | Inst::Memcpy(a, b, _) => {
            v(a);
            v(b);
        }
        Inst::Zero(a, _)
        | Inst::VaStart(a)
        | Inst::VaArg(_, a, _, _)
        | Inst::GotoPtr(a)
        | Inst::Alloca(_, a) => v(a),
        Inst::Overflow(_, _, _, _, _, a, b, rp) => {
            v(a);
            v(b);
            v(rp);
        }
        Inst::Lea(..)
        | Inst::FunAddr(..)
        | Inst::LabelAddr(..)
        | Inst::VaArea(..) => {}
        Inst::Call(_, c, args, _) => {
            if let Callee::Ptr(p) = c {
                v(p)
            }
            for a in args {
                v(a)
            }
        }
        Inst::CallX(_, c, args, _, _) => {
            if let Callee::Ptr(p) = c {
                v(p)
            }
            for (a, _) in args {
                v(a)
            }
        }
        Inst::Sync(_, _, args, _, _) => {
            for a in args {
                v(a)
            }
        }
        Inst::Asm(_, ops) => {
            for op in ops {
                if let Some(x) = &op.inp {
                    v(x)
                }
                if let Some(x) = &op.wb {
                    v(x)
                }
            }
        }
    }
}

/// Gom mọi Tmp mà terminator DÙNG.
pub(crate) fn term_uses(t: &Term, out: &mut Vec<Tmp>) {
    let mut v = |x: &Val| {
        if let Val::Tmp(t) = x {
            out.push(*t)
        }
    };
    match t {
        Term::Br(c, ..) => v(c),
        Term::Ret(Some(r)) => v(r),
        Term::Jmp(_) | Term::Ret(None) | Term::Unreachable => {}
    }
}

/// Đích block của một terminator (để kiểm ref-integrity CFG).
pub(crate) fn term_targets(t: &Term, out: &mut Vec<BlockId>) {
    match t {
        Term::Jmp(b) => out.push(*b),
        Term::Br(_, a, b) => {
            out.push(*a);
            out.push(*b);
        }
        Term::Ret(_) | Term::Unreachable => {}
    }
}

/// Kiểm well-formedness một hàm. Trả Err(mô tả) tại vi phạm ĐẦU TIÊN.
pub fn verify(f: &IrFunc) -> Result<(), String> {
    // (V3) entry tồn tại
    if f.blocks.is_empty() {
        return Err(format!("{}: hàm rỗng (không có entry block)", f.name));
    }
    let nt = f.temps.len() as u32;
    let nb = f.blocks.len() as u32;

    // Tập temp được định nghĩa. Param KHÔNG là temp (sống trong slot khung, backend
    // đổ theo ABI) → mọi đọc param là Load(mem)→temp mới, không có use-before-def.
    let mut defined = vec![false; nt as usize];
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = inst_def(i) {
                if d >= nt {
                    return Err(format!("{}: def temp t{d} ngoài bảng |temps|={nt}", f.name));
                }
                defined[d as usize] = true;
            }
        }
    }

    // (V1) ref-integrity + (V2) def-coverage.
    let mut uses = Vec::new();
    let mut targets = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            uses.clear();
            inst_uses(i, &mut uses);
            for &u in &uses {
                if u >= nt {
                    return Err(format!("{}: dùng temp t{u} ngoài bảng |temps|={nt}", f.name));
                }
                if !defined[u as usize] {
                    return Err(format!("{}: temp t{u} dùng nhưng không đâu định nghĩa", f.name));
                }
            }
        }
        uses.clear();
        term_uses(&b.term, &mut uses);
        for &u in &uses {
            if u >= nt {
                return Err(format!("{}: term block{bi} dùng t{u} ngoài |temps|={nt}", f.name));
            }
            if !defined[u as usize] {
                return Err(format!("{}: term block{bi} dùng t{u} chưa định nghĩa", f.name));
            }
        }
        targets.clear();
        term_targets(&b.term, &mut targets);
        for &tg in &targets {
            if tg >= nb {
                return Err(format!("{}: block{bi} nhảy tới block{tg} ngoài |blocks|={nb}", f.name));
            }
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Lowering AST → IR (IR.md §4) — CHỖ correctness sống. Mô hình -O0 cổ điển: mọi
// biến C nằm trong BỘ NHỚ khung (offset đã do parser cấp); temp chỉ giữ giá trị
// trung gian của biểu thức. lower_expr(n) phát lệnh, trả Val chứa kết quả;
// lower_addr(n) trả Val = ĐỊA CHỈ của một lvalue; lower_stmt(n) dệt block+CFG.
// Đuôi exotic (va/atomic/asm/nested/struct/Switch/goto) → Inst::Opaque(node) tạm,
// backend bridge re-emit đường cũ (step 3 thay dần bằng lowering thật).
// ─────────────────────────────────────────────────────────────────────────────

/// Element CUỐI của stmt-expr có phải câu-LỆNH VÔ-GIÁ-TRỊ (→ value = void) không?
/// Node::Block KHÔNG thuộc đây: `{…}` compound-statement cuối là void nhưng
/// `({…})` stmt-expr LỒNG mang value — hai cái CÙNG Node::Block, không phân biệt
/// được bằng kiểu node, nên luôn recurse qua lower_expr (Block arm) để propagate
/// value nếu có; value thừa của compound-statement vô hại (không ai đọc).
/// Mirror phần còn lại các arm tường minh của lower_stmt; đồng bộ khi thêm stmt mới.
fn is_stmt_node(n: &Node) -> bool {
    matches!(
        n,
        Node::Ret(_)
            | Node::If(..)
            | Node::While(..)
            | Node::For(..)
            | Node::Do(..)
            | Node::Break
            | Node::Continue
            | Node::Switch(..)
            | Node::Case(_)
            | Node::Label(..)
            | Node::Goto(_)
            | Node::GotoPtr(_)
    )
}

fn map_op(s: &str) -> Op {
    match s {
        "+" => Op::Add, "-" => Op::Sub, "*" => Op::Mul, "/" => Op::Div, "%" => Op::Rem,
        "&" => Op::And, "|" => Op::Or, "^" => Op::Xor, "<<" => Op::Shl, ">>" => Op::Shr,
        "==" => Op::Eq, "!=" => Op::Ne, "<" => Op::Lt, "<=" => Op::Le, ">" => Op::Gt, ">=" => Op::Ge,
        _ => unreachable!("op không phải nhị phân: {s}"),
    }
}

struct Lower<'a> {
    a: &'a Ast,
    temps: Vec<TypeId>,
    blocks: Vec<Block>,
    cur: usize,
    done: bool, // block hiện tại đã có terminator?
    brk: Vec<BlockId>,
    cont: Vec<BlockId>,
    case_blk: HashMap<u32, BlockId>, // Case-node id → block (đích LC trong switch)
    label_blk: HashMap<String, BlockId>, // tên goto-label → block (lazy, forward/back)
}

impl<'a> Lower<'a> {
    fn t(&mut self, ty: TypeId) -> Tmp {
        self.temps.push(ty);
        (self.temps.len() - 1) as Tmp
    }
    fn push(&mut self, i: Inst) {
        if !self.done {
            self.blocks[self.cur].insts.push(i);
        }
    }
    fn reserve(&mut self) -> BlockId {
        self.blocks.push(Block { insts: Vec::new(), term: Term::Unreachable });
        (self.blocks.len() - 1) as BlockId
    }
    fn goto(&mut self, b: BlockId) {
        self.cur = b as usize;
        self.done = false;
    }
    fn seal(&mut self, term: Term) {
        if !self.done {
            self.blocks[self.cur].term = term;
            self.done = true;
        }
    }
    fn label_block(&mut self, name: String) -> BlockId {
        if let Some(&b) = self.label_blk.get(&name) {
            b
        } else {
            let b = self.reserve();
            self.label_blk.insert(name, b);
            b
        }
    }
    fn scalar(&self, ty: TypeId) -> bool {
        let tt = &self.a.tt;
        tt.is_integer(ty) || tt.is_float(ty) || matches!(tt.tys[ty as usize], Ty::Ptr(_))
    }

    /// Địa chỉ của một lvalue → Val (kiểu địa chỉ giữ ULONG, 64-bit không wrap).
    fn lower_addr(&mut self, n: NodeId) -> Val {
        let a = self.a; // &'a Ast (Copy) — tách khỏi &mut self
        match &a.nodes[n as usize] {
            Node::Var(off) => {
                let off = *off;
                let t = self.t(ULONG);
                self.push(Inst::Lea(t, Place::Local(off)));
                Val::Tmp(t)
            }
            Node::GVar(i) => {
                let name = a.globals[*i as usize].name.clone();
                let t = self.t(ULONG);
                self.push(Inst::Lea(t, Place::Global(name, 0)));
                Val::Tmp(t)
            }
            Node::Member(b, off) => {
                let (b, off) = (*b, *off);
                let base = self.lower_addr(b);
                if off == 0 {
                    base
                } else {
                    let t = self.t(ULONG);
                    self.push(Inst::Bin(t, Op::Add, ULONG, base, Val::Imm(off as i64)));
                    Val::Tmp(t)
                }
            }
            Node::Deref(e) => self.lower_expr(*e),
            Node::Str(i) => {
                let i = *i;
                let t = self.t(ULONG);
                self.push(Inst::Lea(t, Place::Str(i)));
                Val::Tmp(t)
            }
            // lvalue exotic (SRet/Comma/Assign-struct/Cond/Block stmt-expr): giá trị
            // kiểu-aggregate CHÍNH LÀ địa chỉ (mô hình by-ref) — khớp AST-walk
            // addr()=expr(). Scalar-assign-làm-lvalue là invalid C nên không tới đây.
            _ => self.lower_expr(n),
        }
    }

    /// Giá trị (rvalue) của một biểu thức → Val.
    fn lower_expr(&mut self, n: NodeId) -> Val {
        let a = self.a;
        let ty = a.types[n as usize];
        match &a.nodes[n as usize] {
            Node::Num(v) => Val::Imm(*v),
            Node::FNum(f) => Val::FImm(f.to_bits()),
            Node::Str(_) => self.lower_addr(n), // mảng ký tự decay → con trỏ
            Node::Var(_) | Node::GVar(_) | Node::Member(..) | Node::Deref(_) => {
                let addr = self.lower_addr(n);
                if self.scalar(ty) {
                    let t = self.t(ty);
                    self.push(Inst::Load(t, ty, addr));
                    Val::Tmp(t)
                } else {
                    addr // mảng/struct: rvalue = địa chỉ (decay / by-ref)
                }
            }
            Node::Addr(e) => self.lower_addr(*e),
            Node::Assign(l, r) => {
                let (l, r) = (*l, *r);
                let lty = a.types[l as usize];
                if self.scalar(lty) {
                    let addr = self.lower_addr(l);
                    let v = self.lower_expr(r);
                    self.push(Inst::Store(lty, addr, v));
                    v
                } else {
                    // Gán struct/union: copy size(ty) byte; rvalue = địa chỉ đích
                    // (C99 6.5.16). LHS-addr trước RHS (khớp thứ tự AST reference).
                    let dst = self.lower_addr(l);
                    let src = self.lower_expr(r);
                    self.push(Inst::Memcpy(dst, src, a.tt.size(lty)));
                    dst
                }
            }
            Node::Bin(op, l, r) => {
                let (op, l, r) = (*op, *l, *r);
                let opty = a.types[l as usize]; // kiểu chung sau UAC (parser đã cast)
                // Sinh vế PHẢI trước (khớp đường AST — C99 6.5p3 để thứ tự operand
                // unspecified, nhưng khớp reference thì differential không nhiễu:
                // vd `x[i] |= foo()` cần foo() chạy trước khi đọc x[i]). Vị trí toán
                // hạng giữ nguyên x=lhs, y=rhs; chỉ đổi thứ tự side-effect.
                let y = self.lower_expr(r);
                let x = self.lower_expr(l);
                let t = self.t(ty);
                self.push(Inst::Bin(t, map_op(op), opty, x, y));
                Val::Tmp(t)
            }
            Node::Neg(e) => {
                let v = self.lower_expr(*e);
                let t = self.t(ty);
                self.push(Inst::Un(t, Un::Neg, ty, v));
                Val::Tmp(t)
            }
            Node::Cast(e) => {
                let e = *e;
                let from = a.types[e as usize];
                let v = self.lower_expr(e);
                if from == ty || !self.scalar(from) || !self.scalar(ty) {
                    v // reinterpret / no-op (kể cả tới/từ struct-ptr)
                } else {
                    let t = self.t(ty);
                    self.push(Inst::Cast(t, from, ty, v));
                    Val::Tmp(t)
                }
            }
            Node::Cond(c, tb, eb) => {
                let (c, tb, eb) = (*c, *tb, *eb);
                let cv = self.lower_expr(c);
                let (tblk, eblk, mblk) = (self.reserve(), self.reserve(), self.reserve());
                let res = self.t(ty);
                self.seal(Term::Br(cv, tblk, eblk));
                self.goto(tblk);
                let tv = self.lower_expr(tb);
                self.push(Inst::Copy(res, ty, tv));
                self.seal(Term::Jmp(mblk));
                self.goto(eblk);
                let ev = self.lower_expr(eb);
                self.push(Inst::Copy(res, ty, ev));
                self.seal(Term::Jmp(mblk));
                self.goto(mblk);
                Val::Tmp(res)
            }
            Node::Comma(l, r) => {
                let (l, r) = (*l, *r);
                self.lower_expr(l);
                self.lower_expr(r)
            }
            Node::Post(op, l, delta) => {
                let (op, l, delta) = (*op, *l, *delta);
                let lty = a.types[l as usize];
                let addr = self.lower_addr(l);
                let old = self.t(lty);
                self.push(Inst::Load(old, lty, addr));
                let nw = self.t(lty);
                let o = if op == "+" { Op::Add } else { Op::Sub };
                self.push(Inst::Bin(nw, o, lty, Val::Tmp(old), Val::Imm(delta)));
                self.push(Inst::Store(lty, addr, Val::Tmp(nw)));
                Val::Tmp(old)
            }
            // Call thuần-scalar → Inst::Call (IR sạch). Có composite arg/ret (struct
            // by-value, HFA, >16B, float≠8B, tràn reg) → ABI-automaton C.1–C.11 →
            // Inst::CallX (operand mang KIỂU, emitter port self.call). ret struct đã
            // được parser bọc SRet nên nhánh này ty luôn scalar/void.
            Node::Call(name, args, nfix) => {
                let (name, nfix) = (name.clone(), *nfix);
                let args = args.clone();
                if self.call_composite(&args, ty) {
                    let cargs = self.lower_call_args(&args);
                    return self.emit_callx(Callee::Sym(name), cargs, ty);
                }
                let av: Vec<Val> = args.iter().map(|&x| self.lower_expr(x)).collect();
                if ty == VOID {
                    self.push(Inst::Call(None, Callee::Sym(name), av, nfix));
                    Val::Imm(0)
                } else {
                    let t = self.t(ty);
                    self.push(Inst::Call(Some(t), Callee::Sym(name), av, nfix));
                    Val::Tmp(t)
                }
            }
            Node::CallPtr(e, args, nfix) => {
                let (e, nfix) = (*e, *nfix);
                let args = args.clone();
                if self.call_composite(&args, ty) {
                    let p = self.lower_expr(e);
                    let cargs = self.lower_call_args(&args);
                    return self.emit_callx(Callee::Ptr(p), cargs, ty);
                }
                let p = self.lower_expr(e);
                let av: Vec<Val> = args.iter().map(|&x| self.lower_expr(x)).collect();
                if ty == VOID {
                    self.push(Inst::Call(None, Callee::Ptr(p), av, nfix));
                    Val::Imm(0)
                } else {
                    let t = self.t(ty);
                    self.push(Inst::Call(Some(t), Callee::Ptr(p), av, nfix));
                    Val::Tmp(t)
                }
            }
            // Call trả struct (parser bọc SRet(call, off, sz)): ABI đầy đủ + gom kết
            // quả (v-reg HFA / x0:x1 ≤16B / x8-sret >16B) về local[off]; giá trị = &local.
            Node::SRet(call, off, _sz) => {
                let (call, off) = (*call, *off);
                let (pe, name, cargs_nodes) = match &a.nodes[call as usize] {
                    Node::Call(nm, ar, _) => (None, Some(nm.clone()), ar.clone()),
                    Node::CallPtr(e, ar, _) => (Some(*e), None, ar.clone()),
                    _ => unreachable!("SRet bọc non-call"),
                };
                let callee = match name {
                    Some(nm) => Callee::Sym(nm),
                    None => Callee::Ptr(self.lower_expr(pe.unwrap())),
                };
                let cargs = self.lower_call_args(&cargs_nodes);
                let d = self.t(ULONG); // địa chỉ struct-result (&local[off])
                self.push(Inst::CallX(Some(d), callee, cargs, ty, off));
                Val::Tmp(d)
            }
            // exotic đã có Inst typed (operand-free) → hạ thẳng, không qua Opaque.
            Node::FunAddr(name) => {
                let name = name.clone();
                let t = self.t(ty);
                self.push(Inst::FunAddr(t, name));
                Val::Tmp(t)
            }
            Node::LabelAddr(name) => {
                let name = name.clone();
                let t = self.t(ty);
                self.push(Inst::LabelAddr(t, name));
                Val::Tmp(t)
            }
            Node::Zero(l, sz) => {
                let (l, sz) = (*l, *sz);
                let addr = self.lower_addr(l);
                self.push(Inst::Zero(addr, sz));
                Val::Imm(0) // void
            }
            Node::VaStart(ap) => {
                let ap = *ap;
                let addr = self.lower_addr(ap);
                self.push(Inst::VaStart(addr));
                Val::Imm(0) // void
            }
            Node::VaArg(ap, vt, tmp) => {
                let (ap, vt, tmp) = (*ap, *vt, *tmp);
                let addr = self.lower_addr(ap);
                let d = self.t(ty);
                self.push(Inst::VaArg(d, addr, vt, tmp));
                Val::Tmp(d)
            }
            Node::Overflow(op, oa, ob, rp) => {
                let (op, oa, ob, rp) = (*op, *oa, *ob, *rp);
                let (ta, tb) = (a.types[oa as usize], a.types[ob as usize]);
                let rt = a.tt.pointee(a.types[rp as usize]).unwrap();
                let (va, vb) = (self.lower_expr(oa), self.lower_expr(ob));
                let vrp = self.lower_expr(rp);
                let d = self.t(ty);
                self.push(Inst::Overflow(d, op, ta, tb, rt, va, vb, vrp));
                Val::Tmp(d)
            }
            Node::VaArea(off) => {
                let off = *off;
                let d = self.t(ty);
                self.push(Inst::VaArea(d, off));
                Val::Tmp(d)
            }
            Node::Alloca(e) => {
                let sz = self.lower_expr(*e);
                let d = self.t(ty);
                self.push(Inst::Alloca(d, sz));
                Val::Tmp(d)
            }
            Node::Sync(op, args, sz) => {
                let (op, sz) = (*op, *sz);
                let args = args.clone();
                let vals: Vec<Val> = args.iter().map(|&x| self.lower_expr(x)).collect();
                if ty == VOID {
                    self.push(Inst::Sync(None, op, vals, sz, ty));
                    Val::Imm(0)
                } else {
                    let d = self.t(ty);
                    self.push(Inst::Sync(Some(d), op, vals, sz, ty));
                    Val::Tmp(d)
                }
            }
            // EXT(gcc) inline asm (void): materialize từng operand → Val. mem = địa chỉ;
            // pure output = None (chỉ ghi ngược); input/rw = giá trị (+ địa chỉ wb nếu out).
            Node::Asm(tpl, ops) => {
                let (tpl, ops) = (tpl.clone(), ops.clone());
                let irops: Vec<AsmIrOp> = ops
                    .iter()
                    .map(|op| {
                        let oty = a.types[op.e as usize];
                        let (inp, wb) = if op.mem {
                            (Some(self.lower_addr(op.e)), None)
                        } else if op.out && !op.rw {
                            (None, Some(self.lower_addr(op.e)))
                        } else {
                            let v = self.lower_expr(op.e);
                            let wb = op.out.then(|| self.lower_addr(op.e));
                            (Some(v), wb)
                        };
                        AsmIrOp {
                            out: op.out,
                            rw: op.rw,
                            mem: op.mem,
                            fp: op.fp,
                            tied: op.tied,
                            pin: op.pin,
                            ty: oty,
                            inp,
                            wb,
                        }
                    })
                    .collect();
                self.push(Inst::Asm(tpl, irops));
                Val::Imm(0) // asm expr = void
            }
            // EXT(gcc): statement-expression `({ s1; …; last })` ở vị trí biểu thức: chạy
            // tuần tự các stmt (side-effect qua lower_stmt), giá trị = giá trị của
            // stmt CUỐI nếu nó là expr-statement (C99-EXT gcc), ngược lại void.
            // KHÔNG cần Inst mới — stmt-expr chỉ là "statements + 1 value" trong IR.
            Node::Block(v) => {
                let v = v.clone();
                let Some((&last, init)) = v.split_last() else {
                    return Val::Imm(0); // `({ })` — void
                };
                for &s in init {
                    self.lower_stmt(s);
                }
                if is_stmt_node(&a.nodes[last as usize]) {
                    self.lower_stmt(last);
                    Val::Imm(0)
                } else {
                    if self.done {
                        // stmt cuối nằm trong dead-code (stmt trước seal terminator):
                        // mở block tươi để value-expr lower nhất quán (mọi def push đủ),
                        // tránh temp mồ côi (cùng lớp bug orphan-temp lower_stmt).
                        let d = self.reserve();
                        self.goto(d);
                    }
                    self.lower_expr(last)
                }
            }
            // Mọi node biểu-thức C99 đã có arm typed (chứng cứ: probe 0 bridge-hit trên
            // 3748 file thật). Node câu-LỆNH không lọt vào lower_expr (đi lower_stmt).
            _ => unreachable!("lower_expr: node không phải biểu thức đã seal"),
        }
    }

    /// Lower từng arg → (Val, kiểu). Val = giá trị scalar / ĐỊA CHỈ struct (khớp
    /// self.expr): emitter ABI đọc kiểu để phân slot, đọc Val để nạp thanh ghi/stack.
    fn lower_call_args(&mut self, args: &[NodeId]) -> Vec<(Val, TypeId)> {
        args.iter().map(|&x| (self.lower_expr(x), self.a.types[x as usize])).collect()
    }

    /// Push CallX (composite, ret scalar/void) và trả Val kết quả.
    fn emit_callx(&mut self, callee: Callee, cargs: Vec<(Val, TypeId)>, ty: TypeId) -> Val {
        if ty == VOID {
            self.push(Inst::CallX(None, callee, cargs, VOID, 0));
            Val::Imm(0)
        } else {
            let d = self.t(ty);
            self.push(Inst::CallX(Some(d), callee, cargs, ty, 0));
            Val::Tmp(d)
        }
    }


    /// Call cần bridge sang self.call (ABI automaton C.1–C.11) thay vì Inst::Call
    /// thuần-scalar? Đúng khi: (a) return composite, (b) có tham số composite
    /// (struct by-value/HFA/>16B), hoặc (c) tràn thanh ghi (GP>8 hay FP>8 → arg
    /// phải xuống stack). ir_call chỉ lo ca ≤8 GP + ≤8 FP scalar.
    fn call_composite(&self, args: &[NodeId], ret: TypeId) -> bool {
        let a = self.a;
        if ret != VOID && !self.scalar(ret) {
            return true;
        }
        let (mut gp, mut fp) = (0u32, 0u32);
        for &x in args {
            let ty = a.types[x as usize];
            if !self.scalar(ty) {
                return true;
            }
            if a.tt.is_float(ty) {
                // ir_call truyền float qua d-reg (f64) — chỉ đúng cho `double` (8B).
                // `float` (4B → s-reg fcvt) và `long double` (16B → q-reg) cần
                // narrow ABI riêng → bridge sang self.call.
                if a.tt.size(ty) != 8 {
                    return true;
                }
                fp += 1;
            } else {
                gp += 1;
            }
        }
        gp > 8 || fp > 8
    }

    fn lower_stmt(&mut self, n: NodeId) {
        // Code CHẾT sau terminator (return/goto/break/continue) vẫn được Block lower.
        // `push` drop-khi-done nhưng `t()` cấp temp vô điều kiện → một Cond bên trong
        // dead-code `goto` hồi sinh block ⟹ def của addr rớt (done) nhưng use sống lại
        // → temp mồ côi (csmith c0041/c0126, verify V2 bắt). Fix: mở block TƯƠI cho mỗi
        // stmt chết để nó lower NHẤT QUÁN (mọi def push đủ); block unreachable, well-formed,
        // backend emit vô hại; nhãn goto-đích vẫn reachable qua label_block.
        if self.done {
            let d = self.reserve();
            self.goto(d);
        }
        let a = self.a;
        match &a.nodes[n as usize] {
            Node::Block(v) => {
                let v = v.clone();
                for s in v {
                    self.lower_stmt(s);
                }
            }
            Node::Ret(e) => {
                let r = e.map(|e| self.lower_expr(e));
                self.seal(Term::Ret(r));
            }
            Node::If(c, t, e) => {
                let (c, t, e) = (*c, *t, *e);
                let cv = self.lower_expr(c);
                let tblk = self.reserve();
                let mblk = self.reserve();
                let eblk = if e.is_some() { self.reserve() } else { mblk };
                self.seal(Term::Br(cv, tblk, eblk));
                self.goto(tblk);
                self.lower_stmt(t);
                self.seal(Term::Jmp(mblk));
                if let Some(e) = e {
                    self.goto(eblk);
                    self.lower_stmt(e);
                    self.seal(Term::Jmp(mblk));
                }
                self.goto(mblk);
            }
            Node::While(c, b) => {
                let (c, b) = (*c, *b);
                let (cblk, bblk, mblk) = (self.reserve(), self.reserve(), self.reserve());
                self.seal(Term::Jmp(cblk));
                self.goto(cblk);
                let cv = self.lower_expr(c);
                self.seal(Term::Br(cv, bblk, mblk));
                self.cont.push(cblk);
                self.brk.push(mblk);
                self.goto(bblk);
                self.lower_stmt(b);
                self.seal(Term::Jmp(cblk));
                self.cont.pop();
                self.brk.pop();
                self.goto(mblk);
            }
            Node::For(i, c, nx, b) => {
                let (i, c, nx, b) = (*i, *c, *nx, *b);
                if let Some(i) = i {
                    self.lower_stmt(i);
                }
                let (cblk, bblk, nblk, mblk) =
                    (self.reserve(), self.reserve(), self.reserve(), self.reserve());
                self.seal(Term::Jmp(cblk));
                self.goto(cblk);
                match c {
                    Some(c) => {
                        let cv = self.lower_expr(c);
                        self.seal(Term::Br(cv, bblk, mblk));
                    }
                    None => self.seal(Term::Jmp(bblk)),
                }
                self.cont.push(nblk);
                self.brk.push(mblk);
                self.goto(bblk);
                self.lower_stmt(b);
                self.seal(Term::Jmp(nblk));
                self.cont.pop();
                self.brk.pop();
                self.goto(nblk);
                if let Some(nx) = nx {
                    self.lower_expr(nx);
                }
                self.seal(Term::Jmp(cblk));
                self.goto(mblk);
            }
            Node::Do(b, c) => {
                let (b, c) = (*b, *c);
                let (bblk, cblk, mblk) = (self.reserve(), self.reserve(), self.reserve());
                self.seal(Term::Jmp(bblk));
                self.cont.push(cblk);
                self.brk.push(mblk);
                self.goto(bblk);
                self.lower_stmt(b);
                self.seal(Term::Jmp(cblk));
                self.cont.pop();
                self.brk.pop();
                self.goto(cblk);
                let cv = self.lower_expr(c);
                self.seal(Term::Br(cv, bblk, mblk));
                self.goto(mblk);
            }
            Node::Break => {
                let m = *self.brk.last().unwrap();
                self.seal(Term::Jmp(m));
            }
            Node::Continue => {
                let c = *self.cont.last().unwrap();
                self.seal(Term::Jmp(c));
            }
            // Switch: dispatch = chuỗi test range (v-lo ≤u hi-lo) → Br tới case-block;
            // thân giữ nguyên thứ tự (fall-through), Case = ranh giới block, Break→merge.
            Node::Switch(c, b, cases, def) => {
                let (c, b) = (*c, *b);
                let cases = cases.clone();
                let def = *def;
                let cv = self.lower_expr(c);
                let merge = self.reserve();
                for &(_, _, cid) in &cases {
                    let blk = self.reserve();
                    self.case_blk.insert(cid, blk);
                }
                let defblk = match def {
                    Some(d) => {
                        let blk = self.reserve();
                        self.case_blk.insert(d, blk);
                        blk
                    }
                    None => merge,
                };
                for &(lo, hi, cid) in &cases {
                    let target = self.case_blk[&cid];
                    let d1 = self.t(ULONG);
                    self.push(Inst::Bin(d1, Op::Sub, ULONG, cv, Val::Imm(lo)));
                    let cond = self.t(INT);
                    self.push(Inst::Bin(
                        cond,
                        Op::Le,
                        ULONG,
                        Val::Tmp(d1),
                        Val::Imm(hi.wrapping_sub(lo)),
                    ));
                    let next = self.reserve();
                    self.seal(Term::Br(Val::Tmp(cond), target, next));
                    self.goto(next);
                }
                self.seal(Term::Jmp(defblk));
                let body = self.reserve(); // câu lệnh trước case đầu = bất khả đạt (C)
                self.goto(body);
                self.brk.push(merge);
                self.lower_stmt(b);
                self.seal(Term::Jmp(merge));
                self.brk.pop();
                self.goto(merge);
            }
            Node::Case(st) => {
                let st = *st;
                let blk = self.case_blk[&n]; // id node Case = khoá (đã reserve ở Switch)
                self.seal(Term::Jmp(blk)); // fall-through vào nhãn case
                self.goto(blk);
                self.lower_stmt(st);
            }
            Node::Label(name, st) => {
                let (name, st) = (name.clone(), *st);
                let blk = self.label_block(name);
                self.seal(Term::Jmp(blk));
                self.goto(blk);
                self.lower_stmt(st);
            }
            Node::Goto(name) => {
                let blk = self.label_block(name.clone());
                self.seal(Term::Jmp(blk));
            }
            // computed goto / non-local goto: còn exotic (đuôi bước 2)
            Node::GotoPtr(e) => {
                let e = *e;
                let target = self.lower_expr(e);
                self.push(Inst::GotoPtr(target));
            }
            // biểu thức dùng làm câu lệnh: phát side effect, bỏ kết quả
            _ => {
                self.lower_expr(n);
            }
        }
    }
}

/// AST → danh sách IrFunc. Backend chỉ đọc kết quả này + TyTab.
pub fn lower(ast: &Ast) -> Vec<IrFunc> {
    let mut out = Vec::with_capacity(ast.funcs.len());
    for f in &ast.funcs {
        let mut lo = Lower {
            a: ast,
            temps: Vec::new(),
            blocks: vec![Block { insts: Vec::new(), term: Term::Unreachable }],
            cur: 0,
            done: false,
            brk: Vec::new(),
            cont: Vec::new(),
            case_blk: HashMap::new(),
            label_blk: HashMap::new(),
        };
        // Param KHÔNG cần prologue trong IR: backend emit_params đổ arg vào slot
        // khung theo ABI (scalar/float/struct/HFA/variadic) TRƯỚC khi body chạy;
        // body đọc mọi biến (kể cả param) qua Var(off)→Load — mô hình -O0 nhất quán.
        lo.lower_stmt(f.body);
        lo.seal(Term::Ret(None)); // rơi khỏi thân (void; main→0 do frontend chèn)
        let mut labels: Vec<(String, BlockId)> = lo.label_blk.into_iter().collect();
        labels.sort(); // deterministic (HashMap iter order không ổn định)
        out.push(IrFunc {
            name: f.name.clone(),
            temps: lo.temps,
            params: f.params.clone(),
            blocks: lo.blocks,
            frame: f.frame,
            ret: f.ret,
            labels,
        });
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Ngữ nghĩa số học CORE — HÀM NGHĨA nguyên tử, dùng CHUNG bởi interp (proof-side)
// và const-fold (opt.rs, release). MỘT định nghĩa duy nhất ⟹ folder và interpreter
// KHÔNG THỂ lệch: đây là điều kiện faithfulness của correctness-by-construction —
// ⟦fold(Bin op a b)⟧ = ⟦Bin op a b⟧ theo đúng nghĩa vì fold GỌI CHÍNH eval_bin.
// (THEORY.md Phần I §A4 partial-evaluation + §III keystone.)
// ─────────────────────────────────────────────────────────────────────────────

/// Chuẩn hoá giá trị nguyên về đúng bề rộng+dấu của `ty` (ℤ/2^n): định lý "số học
/// int wrap tại size*8 bit" — chính là `ext(ct)` của backend. Float: bit pattern đi thẳng.
pub(crate) fn canon(tt: &TyTab, ty: TypeId, v: i64) -> i64 {
    if tt.is_float(ty) {
        return v;
    }
    let sz = tt.size(ty);
    if sz >= 8 {
        return v;
    }
    let bits = sz * 8;
    let masked = (v as u64) & ((1u64 << bits) - 1);
    if tt.is_unsigned(ty) {
        masked as i64
    } else {
        let sh = 64 - bits; // sign-extend từ `bits`
        ((masked << sh) as i64) >> sh
    }
}

/// ⟦Bin⟧: Op thuần diễn giải trong cấu trúc đại số của `ty` (ℝ nếu float, ℤ/2^n với
/// dấu nếu int). So sánh → {0,1}. Err tại UB (chia/mod 0) → const-fold PHẢI bỏ qua
/// (không fold UB thành hằng: giữ nguyên lệnh, để runtime giữ hành vi target).
pub(crate) fn eval_bin(tt: &TyTab, op: Op, ty: TypeId, x: i64, y: i64) -> Result<i64, String> {
    if tt.is_float(ty) {
        let (a, b) = (f64::from_bits(x as u64), f64::from_bits(y as u64));
        let r = match op {
            Op::Add => a + b,
            Op::Sub => a - b,
            Op::Mul => a * b,
            Op::Div => a / b,
            Op::Eq => return Ok((a == b) as i64),
            Op::Ne => return Ok((a != b) as i64),
            Op::Lt => return Ok((a < b) as i64),
            Op::Le => return Ok((a <= b) as i64),
            Op::Gt => return Ok((a > b) as i64),
            Op::Ge => return Ok((a >= b) as i64),
            _ => return Err("eval_bin: op không hợp lệ trên float".into()),
        };
        return Ok(r.to_bits() as i64);
    }
    let u = tt.is_unsigned(ty);
    let r = match op {
        Op::Add => x.wrapping_add(y),
        Op::Sub => x.wrapping_sub(y),
        Op::Mul => x.wrapping_mul(y),
        Op::Div if y == 0 => return Err("eval_bin: chia 0 (UB)".into()),
        Op::Rem if y == 0 => return Err("eval_bin: mod 0 (UB)".into()),
        Op::Div if u => ((x as u64) / (y as u64)) as i64,
        Op::Div => x.wrapping_div(y),
        Op::Rem if u => ((x as u64) % (y as u64)) as i64,
        Op::Rem => x.wrapping_rem(y),
        Op::And => x & y,
        Op::Or => x | y,
        Op::Xor => x ^ y,
        Op::Shl => x.wrapping_shl(y as u32),
        Op::Shr if u => ((x as u64) >> (y as u32)) as i64,
        Op::Shr => x >> (y as u32),
        Op::Eq => return Ok((x == y) as i64),
        Op::Ne => return Ok((x != y) as i64),
        Op::Lt => return Ok((if u { (x as u64) < (y as u64) } else { x < y }) as i64),
        Op::Le => return Ok((if u { (x as u64) <= (y as u64) } else { x <= y }) as i64),
        Op::Gt => return Ok((if u { (x as u64) > (y as u64) } else { x > y }) as i64),
        Op::Ge => return Ok((if u { (x as u64) >= (y as u64) } else { x >= y }) as i64),
    };
    Ok(canon(tt, ty, r))
}

/// ⟦Cast⟧: chuyển giữa các miền (trunc/ext int, i↔f). _Bool normalize 0/1 (C99
/// 6.3.1.2 / 6.3.1.4). Tổng (không UB) → const-fold luôn fold được cast hằng.
pub(crate) fn eval_cast(tt: &TyTab, from: TypeId, to: TypeId, v: i64) -> i64 {
    let is_bool = matches!(tt.tys[to as usize], Ty::Bool);
    match (tt.is_float(from), tt.is_float(to)) {
        (false, false) => {
            if is_bool {
                (v != 0) as i64
            } else {
                canon(tt, to, v)
            }
        }
        (false, true) => {
            let f = if tt.is_unsigned(from) { (v as u64) as f64 } else { v as f64 };
            f.to_bits() as i64
        }
        (true, false) => {
            let f = f64::from_bits(v as u64);
            if is_bool {
                (f != 0.0) as i64
            } else {
                canon(tt, to, f as i64) // trunc về 0 (C99 6.3.1.4)
            }
        }
        (true, true) => v, // f64 canonical cả hai
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Interp — REFERENCE SEMANTICS ⟦·⟧ của IR CORE (NẤC-1). Đây là HÀM NGHĨA hình thức
// hoá: định nghĩa "IR này TÍNH RA gì", làm ground-truth cho commuting-square oracle
// (pass đúng ⟺ giao hoán với interp). **Định nghĩa toán học đầy đủ mỗi Inst +
// định lý commuting-square: `SEMANTICS.md`** (spec map 1-1 với code dưới đây; mọi
// arm `match inst`/`match term` = một rule §4/§4b, mọi hàm nghĩa nguyên tử §3).
// test-side (#[cfg(test)]) — KHÔNG vào binary release, KHÔNG tính trần 10k (IR.md
// §7.1); là proof-checker, không phải logic compiler. Chưa machine-checked proof —
// mechanized reference semantics ĐƯỢC KIỂM bằng vét-cạn-cấu-trúc (nền cho nấc-2/3).
//
// Trạng thái máy Σ = ⟨ρ, μ⟩ (SEMANTICS.md §2): ρ: Tmp→𝕍 register file (mỗi temp
// một giá trị 64-bit canonical — int sign/zero-extend đúng kiểu, float = BIT
// PATTERN f64, float nâng lên double); μ: [0,frame)→Byte bộ nhớ local phẳng
// (little-endian LP64), Lea(Local off) → index = frame−off. Observable = giá trị
// TRẢ. Global/Str/exotic KHÔNG mô hình hoá → Err = ⊥ (hàm "không thuần", NGOÀI
// không gian CORE ⟹ commuting-square SKIP như UB).
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ast::{TyTab, TypeId, DOUBLE, INT, UINT};

    fn load_mem(mem: &[u8], addr: i64, sz: u32) -> u64 {
        let a = addr as usize;
        let mut x = 0u64;
        for k in 0..sz as usize {
            x |= (mem[a + k] as u64) << (k * 8);
        }
        x
    }
    fn store_mem(mem: &mut [u8], addr: i64, sz: u32, val: u64) {
        let a = addr as usize;
        for k in 0..sz as usize {
            mem[a + k] = ((val >> (k * 8)) & 0xff) as u8;
        }
    }

    /// Chạy IR: interp(entry, args) → giá trị trả. Đệ quy qua Call(Sym).
    pub(crate) fn interp(tt: &TyTab, funcs: &[IrFunc], entry: &str, args: &[i64]) -> Result<i64, String> {
        interp_d(tt, funcs, entry, args, 0)
    }

    /// interp có ĐẾM ĐỘ SÂU: interp dùng Rust-recursion cho Call, nên input ép đệ quy
    /// lớn (vd fact(2^31)) làm tràn stack HOST — không phải bug IR. Chạm trần → Err
    /// (input "ngoài không gian mô hình được" → equiv SKIP, giống UB). Trần chọn thấp
    /// hơn stack thật nhiều lần để an toàn.
    fn interp_d(tt: &TyTab, funcs: &[IrFunc], entry: &str, args: &[i64], depth: u32) -> Result<i64, String> {
        if depth > 500 {
            return Err("interp: đệ quy quá sâu (input ngoài không gian mô hình)".into());
        }
        let f = funcs
            .iter()
            .find(|f| f.name == entry)
            .ok_or_else(|| format!("interp: không thấy hàm {entry}"))?;
        let mut reg = vec![0i64; f.temps.len()];
        // Seed arg vào slot khung (backend emit_params làm điều tương tự theo ABI):
        // param tại offset off ⇒ index mem = frame - off (khớp Lea Local bên dưới).
        let mut mem = vec![0u8; f.frame as usize];
        for (i, &(off, pty)) in f.params.iter().enumerate() {
            let v = canon(tt, pty, *args.get(i).unwrap_or(&0));
            store_mem(&mut mem, (f.frame - off) as i64, tt.size(pty), v as u64);
        }
        let fetch = |reg: &[i64], v: &Val| -> i64 {
            match v {
                Val::Tmp(t) => reg[*t as usize],
                Val::Imm(x) => *x,
                Val::FImm(b) => *b as i64,
            }
        };

        let mut cur = 0usize;
        let mut budget = 10_000_000u64; // chốt chống lặp vô hạn (test nhỏ)
        loop {
            let b = &f.blocks[cur];
            for inst in &b.insts {
                budget -= 1;
                if budget == 0 {
                    return Err("interp: vượt ngân sách bước (lặp vô hạn?)".into());
                }
                match inst {
                    Inst::Bin(d, op, ty, a, bb) => {
                        let (x, y) = (fetch(&reg, a), fetch(&reg, bb));
                        reg[*d as usize] = eval_bin(tt, *op, *ty, x, y)?;
                    }
                    Inst::Un(d, op, ty, a) => {
                        let x = fetch(&reg, a);
                        let r = match op {
                            Un::Neg if tt.is_float(*ty) => (-f64::from_bits(x as u64)).to_bits() as i64,
                            Un::Neg => canon(tt, *ty, x.wrapping_neg()),
                            Un::BNot => canon(tt, *ty, !x),
                        };
                        reg[*d as usize] = r;
                    }
                    Inst::Copy(d, ty, a) => reg[*d as usize] = canon(tt, *ty, fetch(&reg, a)),
                    Inst::Load(d, ty, a) => {
                        let addr = fetch(&reg, a);
                        let sz = tt.size(*ty);
                        let raw = load_mem(&mem, addr, sz);
                        reg[*d as usize] = if tt.is_float(*ty) {
                            if sz == 4 {
                                (f32::from_bits(raw as u32) as f64).to_bits() as i64
                            } else {
                                raw as i64
                            }
                        } else {
                            canon(tt, *ty, raw as i64)
                        };
                    }
                    Inst::Store(ty, a, val) => {
                        let addr = fetch(&reg, a);
                        let sz = tt.size(*ty);
                        let v = fetch(&reg, val);
                        let raw = if tt.is_float(*ty) && sz == 4 {
                            (f64::from_bits(v as u64) as f32).to_bits() as u64
                        } else {
                            v as u64
                        };
                        store_mem(&mut mem, addr, sz, raw);
                    }
                    Inst::Memcpy(d, s, sz) => {
                        let (da, sa) = (fetch(&reg, d) as usize, fetch(&reg, s) as usize);
                        for k in 0..*sz as usize {
                            mem[da + k] = mem[sa + k]; // copy xuôi (khớp backend)
                        }
                    }
                    Inst::Lea(d, p) => match p {
                        // ABI zcc: địa chỉ local = x29 − off; flat-mem [0,frame) với
                        // index 0 = x29−frame ⟹ index = frame − off.
                        Place::Local(off) => reg[*d as usize] = (f.frame - *off) as i64,
                        Place::Global(..) | Place::Str(_) => {
                            return Err("interp: địa chỉ global/str không mô hình hoá".into())
                        }
                    },
                    Inst::Cast(d, from, to, a) => {
                        reg[*d as usize] = eval_cast(tt, *from, *to, fetch(&reg, a));
                    }
                    Inst::Call(d, c, args, _) => {
                        let Callee::Sym(name) = c else {
                            return Err("interp: gọi gián tiếp không mô hình hoá".into());
                        };
                        let av: Vec<i64> = args.iter().map(|v| fetch(&reg, v)).collect();
                        let r = interp_d(tt, funcs, name, &av, depth + 1)?;
                        if let Some(d) = d {
                            reg[*d as usize] = canon(tt, f.temps[*d as usize], r);
                        }
                    }
                    Inst::FunAddr(..)
                    | Inst::LabelAddr(..)
                    | Inst::Zero(..)
                    | Inst::VaStart(..)
                    | Inst::VaArg(..)
                    | Inst::Overflow(..)
                    | Inst::VaArea(..)
                    | Inst::GotoPtr(..)
                    | Inst::Alloca(..)
                    | Inst::CallX(..)
                    | Inst::Sync(..)
                    | Inst::Asm(..) => {
                        return Err("interp: exotic (symbol/va/overflow/goto/alloca/callX/sync/asm — hàm không thuần)".into())
                    }
                }
            }
            match &b.term {
                Term::Jmp(t) => cur = *t as usize,
                Term::Br(c, t, e) => cur = if fetch(&reg, c) != 0 { *t } else { *e } as usize,
                Term::Ret(v) => return Ok(v.map(|v| fetch(&reg, &v)).unwrap_or(0)),
                Term::Unreachable => return Err("interp: chạm Unreachable".into()),
            }
        }
    }

    // Dựng nhanh một IrFunc. params = (offset khung, kiểu) — param sống trong slot.
    pub(crate) fn mk(
        name: &str,
        temps: Vec<TypeId>,
        params: Vec<(u32, TypeId)>,
        frame: u32,
        ret: TypeId,
        blocks: Vec<Block>,
    ) -> IrFunc {
        IrFunc { name: name.into(), temps, params, blocks, frame, ret, labels: vec![] }
    }

    // ── Định lý 1: IR biểu đạt được đệ quy + rẽ nhánh + số học int (factorial).
    // fact(n) = n<=1 ? 1 : n*fact(n-1).  Chứng verify OK + interp(5)=120.
    #[test]
    fn factorial() {
        let tt = TyTab::new();
        // param n ở slot khung off=16 (index frame−off=0). t0=&n, t1=n, t2=cond,
        // t3=n−1, t4=rec, t5=prod. Mô hình mới: param Load từ slot, không param-temp.
        let f = mk(
            "fact",
            vec![ULONG, INT, INT, INT, INT, INT],
            vec![(16, INT)],
            16,
            INT,
            vec![
                Block {
                    insts: vec![
                        Inst::Lea(0, Place::Local(16)),
                        Inst::Load(1, INT, Val::Tmp(0)),
                        Inst::Bin(2, Op::Le, INT, Val::Tmp(1), Val::Imm(1)),
                    ],
                    term: Term::Br(Val::Tmp(2), 1, 2),
                },
                Block { insts: vec![], term: Term::Ret(Some(Val::Imm(1))) },
                Block {
                    insts: vec![
                        Inst::Bin(3, Op::Sub, INT, Val::Tmp(1), Val::Imm(1)),
                        Inst::Call(Some(4), Callee::Sym("fact".into()), vec![Val::Tmp(3)], 1),
                        Inst::Bin(5, Op::Mul, INT, Val::Tmp(1), Val::Tmp(4)),
                    ],
                    term: Term::Ret(Some(Val::Tmp(5))),
                },
            ],
        );
        verify(&f).expect("fact well-formed");
        assert_eq!(interp(&tt, std::slice::from_ref(&f), "fact", &[5]).unwrap(), 120);
        assert_eq!(interp(&tt, std::slice::from_ref(&f), "fact", &[0]).unwrap(), 1);
    }

    // ── Định lý 2: memory tường minh (Lea/Store/Load) round-trip đúng width.
    // frame[0..4] ← 42 (INT); đọc lại + 8 = 50.
    #[test]
    fn load_store() {
        let tt = TyTab::new();
        let f = mk(
            "ls",
            vec![INT, INT, INT], // t0=addr, t1=loaded, t2=result
            vec![],
            8,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Lea(0, Place::Local(8)), // off=frame ⟹ index 0 (x29−frame)
                    Inst::Store(INT, Val::Tmp(0), Val::Imm(42)),
                    Inst::Load(1, INT, Val::Tmp(0)),
                    Inst::Bin(2, Op::Add, INT, Val::Tmp(1), Val::Imm(8)),
                ],
                term: Term::Ret(Some(Val::Tmp(2))),
            }],
        );
        verify(&f).expect("ls well-formed");
        assert_eq!(interp(&tt, std::slice::from_ref(&f), "ls", &[]).unwrap(), 50);
    }

    // ── Định lý 3: số học wrap tại 2^32 cho INT (canon), khác hẳn ULONG/64-bit.
    #[test]
    fn int_wrap() {
        let tt = TyTab::new();
        // INT_MAX + 1 = INT_MIN (wrap 32-bit signed)
        assert_eq!(eval_bin(&tt, Op::Add, INT, i32::MAX as i64, 1).unwrap(), i32::MIN as i64);
        // unsigned: 0u - 1u = 0xFFFFFFFF (UINT)
        assert_eq!(eval_bin(&tt, Op::Sub, UINT, 0, 1).unwrap(), 0xFFFF_FFFF);
        // so sánh unsigned: (uint)-1 > 0  (khác signed -1 < 0)
        assert_eq!(eval_bin(&tt, Op::Gt, UINT, canon(&tt, UINT, -1), 0).unwrap(), 1);
        assert_eq!(eval_bin(&tt, Op::Lt, INT, -1, 0).unwrap(), 1);
    }

    // ── Định lý 4: cast i↔f và _Bool normalize (C99 6.3.1.2 / 6.3.1.4).
    #[test]
    fn casts() {
        let tt = TyTab::new();
        // int 7 → double 7.0
        let d = eval_cast(&tt, INT, DOUBLE, 7);
        assert_eq!(f64::from_bits(d as u64), 7.0);
        // double 3.9 → int 3 (trunc về 0)
        let i = eval_cast(&tt, DOUBLE, INT, (3.9f64).to_bits() as i64);
        assert_eq!(i, 3);
        // int 5 → _Bool 1 ; int 0 → _Bool 0
        use crate::ast::BOOL;
        assert_eq!(eval_cast(&tt, INT, BOOL, 5), 1);
        assert_eq!(eval_cast(&tt, INT, BOOL, 0), 0);
    }

    // ── Lowering thật: parse snippet C → lower → verify → interp (oracle).
    // Chứng commuting-square ở tầng lowering: ⟦lower(AST)⟧ = nghĩa chương trình.
    pub(crate) fn compile(name: &str, src: &str) -> (crate::ast::Ast, Vec<IrFunc>) {
        // Tên file DUY NHẤT theo băm(src): test chạy song song, trùng `name` khác
        // `src` sẽ đua-ghi cùng file → parse rác. Băm src ⟹ khác src = khác file.
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        src.hash(&mut h);
        let path = std::env::temp_dir().join(format!("zcc_ir_{name}_{:x}.c", h.finish()));
        std::fs::write(&path, src).unwrap();
        let (t, l, f) = crate::preprocess::preprocess(path.to_str().unwrap(), &[], &[], &[]).unwrap();
        let ast = crate::parser::parse(&t, &l, &f).unwrap();
        let ir = lower(&ast);
        (ast, ir)
    }
    pub(crate) fn run(name: &str, src: &str, entry: &str, args: &[i64]) -> i64 {
        let (ast, ir) = compile(name, src);
        for f in &ir {
            verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
        }
        interp(&ast.tt, &ir, entry, args).unwrap()
    }

    // ─────────────────────────────────────────────────────────────────────────
    // equiv — GATE chứng pass (commuting-square / translation-validation cơ học).
    // Bất biến quản trị THAY trần LOC (Vu 2026-08-20): pass P đúng ⟺ ∀input,
    // ⟦A⟧(input) = ⟦P(A)⟧(input). Ta không TIN bằng lý luận — ta ĐO bằng interp.
    // ─────────────────────────────────────────────────────────────────────────

    /// Battery input cho differential-interp. arity≤2: **VÉT CẠN** miền nhỏ [-6,6]^n
    /// (exhaustive → complete cho biên value-space nhỏ) + biên INT_MAX/MIN mỗi toạ độ.
    /// arity≥3: LCG deterministic 256 vector (Date/random cấm — deterministic để resume).
    pub(crate) fn battery(arity: usize) -> Vec<Vec<i64>> {
        let bound: [i64; 8] = [0, 1, -1, i32::MAX as i64, i32::MIN as i64, 255, -256, 1000003];
        if arity == 0 {
            return vec![vec![]];
        }
        let mut out: Vec<Vec<i64>> = Vec::new();
        if arity <= 2 {
            let small: Vec<i64> = (-6..=6).collect();
            if arity == 1 {
                for &x in &small {
                    out.push(vec![x]);
                }
                for &x in &bound {
                    out.push(vec![x]);
                }
            } else {
                for &x in &small {
                    for &y in &small {
                        out.push(vec![x, y]);
                    }
                }
                for &b in &bound {
                    out.push(vec![b, 1]); // toạ độ kia = 1 (tránh 0 nuốt phép nhân)
                    out.push(vec![1, b]);
                    out.push(vec![b, b]);
                }
            }
            return out;
        }
        let mut s: u64 = 0x2545F4914F6CDD1D;
        for _ in 0..256 {
            let mut v = Vec::with_capacity(arity);
            for _ in 0..arity {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                v.push((s >> 33) as i64 - (1 << 30));
            }
            out.push(v);
        }
        out
    }

    /// ⟦A⟧ ≡ ⟦B⟧ trên `entry`? Observable = giá trị TRẢ. Quy ước UB (diff-tại-UB vô
    /// nghĩa): before Err ⟹ input ngoài không gian (UB/opaque) → SKIP; before Ok ∧
    /// after Err ⟹ pass phá well-defined → FAIL; cả hai Ok ⟹ so. Anti-vacuous: nếu
    /// KHÔNG input nào so được (hàm toàn opaque/UB) → Err (cấm pass "pass" rỗng).
    pub(crate) fn equiv(tt: &TyTab, a: &[IrFunc], b: &[IrFunc], entry: &str) -> Result<(), String> {
        let fa = a.iter().find(|f| f.name == entry).ok_or("equiv: thiếu entry ở A")?;
        let arity = fa.params.len();
        let mut compared = 0u32;
        for input in battery(arity) {
            match (interp(tt, a, entry, &input), interp(tt, b, entry, &input)) {
                (Ok(x), Ok(y)) => {
                    if x != y {
                        return Err(format!("equiv PHÁ VỠ: {entry}{input:?} → A={x} B={y}"));
                    }
                    compared += 1;
                }
                (Ok(x), Err(e)) => {
                    return Err(format!("equiv: pass biến well-defined→lỗi: {entry}{input:?} A={x} B_err={e}"));
                }
                (Err(_), _) => {} // before ngoài không gian → skip
            }
        }
        if compared == 0 {
            return Err(format!("equiv VACUOUS: {entry} không input nào interp được — pass chưa được chứng"));
        }
        Ok(())
    }

    // Tự-chứng equiv (chứng chính công cụ chứng — luật input-sạch): identity phải
    // equiv; đột biến MỘT Op phải bị bắt. Nếu test này đỏ, mọi verdict pass vô giá trị.
    #[test]
    fn equiv_selfproof() {
        let (ast, ir) = compile("eqv", "int f(int a,int b){return a*b + a - 7;}");
        equiv(&ast.tt, &ir, &ir, "f").expect("identity phải equiv");
        let mut bad = ir.clone();
        let mut mutated = false;
        'outer: for f in bad.iter_mut().filter(|f| f.name == "f") {
            for blk in f.blocks.iter_mut() {
                for inst in blk.insts.iter_mut() {
                    if let Inst::Bin(_, op @ Op::Mul, ..) = inst {
                        *op = Op::Add; // Mul → Add: đột biến ngữ nghĩa
                        mutated = true;
                        break 'outer;
                    }
                }
            }
        }
        assert!(mutated, "phải có Bin(Mul) để đột biến");
        assert!(equiv(&ast.tt, &ir, &bad, "f").is_err(), "đột biến Mul→Add phải bị equiv bắt");
    }

    #[test]
    fn lower_arith() {
        assert_eq!(run("arith", "int f(int a,int b){return a*b+7;}", "f", &[6, 7]), 49);
    }
    #[test]
    fn lower_if() {
        let s = "int mx(int a,int b){if(a>b)return a;return b;}";
        assert_eq!(run("if", s, "mx", &[3, 9]), 9);
        assert_eq!(run("if", s, "mx", &[9, 3]), 9);
    }
    #[test]
    fn lower_struct_assign() {
        // q = p là COPY (Inst::Memcpy, CORE) — sửa q KHÔNG đụng p. interp thực
        // thi được ⟺ struct-assign đã là CORE (Opaque sẽ trả Err → panic).
        // f(3,4): p={3,4}; q=p; q.x+=100 ⟹ 3*1000+4*10+103 = 3143.
        let s = "struct P{int x,y;};int f(int a,int b){struct P p,q;p.x=a;p.y=b;q=p;q.x=q.x+100;return p.x*1000+p.y*10+q.x;}";
        assert_eq!(run("sa", s, "f", &[3, 4]), 3143);
    }
    #[test]
    fn lower_for() {
        let s = "int sum(int n){int s=0;int i;for(i=1;i<=n;i=i+1)s=s+i;return s;}";
        assert_eq!(run("for", s, "sum", &[5]), 15);
    }
    #[test]
    fn lower_while_fib() {
        let s = "int fib(int n){int a=0,b=1,i=0;while(i<n){int t=a+b;a=b;b=t;i=i+1;}return a;}";
        assert_eq!(run("fib", s, "fib", &[10]), 55);
    }
    #[test]
    fn lower_ptr() {
        let s = "int viadr(int x){int y=x;int*p=&y;*p=*p+3;return y;}";
        assert_eq!(run("ptr", s, "viadr", &[10]), 13);
    }
    #[test]
    fn lower_recursion() {
        assert_eq!(run("rec", "int fact(int n){if(n<=1)return 1;return n*fact(n-1);}", "fact", &[5]), 120);
    }

    // ── Định lý 5: verifier BẮT IR hỏng (dùng temp không định nghĩa + block lạc).
    #[test]
    fn verifier_rejects() {
        // dùng t9 (không đâu def) → def-coverage vỡ
        let bad = mk(
            "bad",
            vec![INT],
            vec![],
            0,
            INT,
            vec![Block { insts: vec![], term: Term::Ret(Some(Val::Tmp(9))) }],
        );
        assert!(verify(&bad).is_err());
        // nhảy tới block không tồn tại → ref-integrity CFG vỡ
        let bad2 = mk(
            "bad2",
            vec![INT],
            vec![],
            0,
            INT,
            vec![Block { insts: vec![], term: Term::Jmp(7) }],
        );
        assert!(verify(&bad2).is_err());
    }
}
