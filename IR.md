# zcc IR — bản thiết kế contract (stage 3, ĐỀ XUẤT, chưa gõ code)

> Trạng thái: PLAN chờ Vu duyệt. Stage 1 (cắt Mach-O → ELF-only) + stage 2 (seal
> edu u"/U") đã đóng sổ xanh (14/14). Doc này là quyết định kiến trúc lớn — bàn
> trước khi gõ, không nhảy thẳng vào code.

## 0. Vì sao IR tồn tại (hai lý lẽ, KHÔNG phải tốc độ)

1. **Contract cứng frontend/backend.** Hiện boundary là `ast.rs` (AST + TyTab)
   nhưng AST còn mang ngữ nghĩa NGUỒN C (declarator, lvalue-ness, scope) → backend
   buộc *hiểu C* mới codegen. Thêm target = viết lại per-node walk: công việc
   **O(target × construct)**. IR cắt thành **O(construct)** [frontend→IR] +
   **O(target)** [IR→asm]. Đây là điều kiện để community thêm target = một file.
2. **Mở khoá không gian định lý** mà AST→asm che mất: dataflow analysis, fixpoint
   trên lattice, liveness, "optimization = biến đổi bảo toàn ngữ nghĩa". zcc tự
   nhận "mỗi LOC map một định lý" → thiếu IR là thiếu nguyên mảng.

**Optimization KHOAN — defer (Vu chốt 2026-08-20).** Baseline = CHỈ IR (contract +
tổ chức code + extensibility), KHÔNG pass. Lý do Vu: IR "học thuật + tổ chức + mở
rộng hơn optimization". Pass layer (§5) = tương lai, ngoài baseline. Correctness
sống ở lowering, KHÔNG ở pass.

**Trần 10k GIỮ (Vu đảo lại lần cuối).** Mỗi LOC — mỗi token — map một định lý; toàn
`src/` = vài trăm định lý compile ra 10k Rust. Sự thật LOC: 1 target ⇒ IR **cộng
thêm** code (2 chặng lower); src 8484 → còn ~1516 ngân sách. → **interp + verifier
+ ir.sh = TEST-SIDE** (proof-checker, không phải logic biến đổi ⇒ `tests/`, không
trần). `src/ir.rs` chỉ mang **IR types + AST→IR lowering**; backend IR→asm thay chỗ
AST→asm cũ (không cộng ròng nhiều — backend hết phải "hiểu C").

## 1. Vị trí & bất biến kiến trúc mới

```
lexer → parser → AST (ast.rs) ──lower──▶ IR (ir.rs) ──lower──▶ codegen/<target> → .s
                     │                      │                        │
                  TyTab dùng chung ─────────┴────────────────────────┘  (layout size/align)
```

- **Boundary mới = `src/ir.rs`.** Backend chỉ đọc IR + TyTab; TUYỆT ĐỐI không đọc
  AST/parser. Test contract cơ học: *viết được backend mới CHỈ từ IR spec, không
  đọc một dòng frontend*.
- **TyTab tái dùng nguyên** (không nhân đôi type system). IR value mang `TypeId`.
- Bất biến thay trần 10k: **mỗi pass provable** (phát biểu 4 phần ở §6).
- `codegen/mod.rs` vẫn là cửa duy nhất; chữ ký đổi `emit(&Ir) -> String`.

## 2. IR shape — typed linear 3-address, NON-SSA (đã chốt)

Lý do non-SSA (xem thảo luận): const-fold/peephole local + regalloc linear-scan
KHÔNG cần SSA; liveness trên 3-address là định lý cổ điển gọn. SSA construction
(dom-frontier, phi) + destruction (out-of-SSA: lost-copy/swap) là ổ miscompile,
đắt LOC — để dành, chỉ mở khi có global opt thật đòi (−O0 edu gần như không).

```
Ir            = { funcs: Vec<IrFunc>, globals: …(mượn AST), strs, … }
IrFunc        = { name, params: Vec<(Vloc, TypeId)>, ret: TypeId,
                  temps: Vec<TypeId>,        // bảng kiểu mọi temp t0..tN
                  blocks: Vec<Block>,        // block[0] = entry
                  frame:  …(local offsets, giữ từ parser) }
Block         = { id: BlockId, insts: Vec<Inst>, term: Term }   // term BẮT BUỘC cuối
Val           = Temp(u32) | Imm(i64) | FImm(f64)                // giá trị 3-address
Inst          = Bin(dst, Op, a, b)          // dst = a op b   (op mang kiểu qua dst)
              | Un (dst, Op, a)
              | Load(dst, addr, TypeId)      // đọc bộ nhớ theo width type
              | Store(addr, val, TypeId)
              | Addr(dst, Vloc)              // &local / &global / &param
              | Cast(dst, a, from→to)        // usual-arith / trunc / ext / f↔i
              | Call(dst?, callee, args, nfix)  // nfix = số arg cố định (variadic ABI)
              | Copy(dst, a)
Term          = Jmp(BlockId)
              | Br(cond, BlockId, BlockId)   // if cond
              | Ret(val?)
              | Switch(val, Vec<(i64,BlockId)>, default)
```

Đặc điểm: **phẳng** (không lồng expr — parser đã hạ cây), **có kiểu** (mọi Temp có
`TypeId`), **explicit control flow** (block + terminator), **memory tường minh**
(Load/Store — không lvalue ngầm). Backend chỉ còn việc *chọn lệnh per-inst* +
*regalloc* + *ABI* — không còn hiểu C.

Op set (§ test-first): mua khi lowering một construct AST đòi; danh sách khởi điểm
= { add sub mul div mod, and or xor shl shr, cmp{eq ne lt le gt ge} (signed/uns),
neg not, fadd… fcmp… } — mỗi op một dòng match ở interp + mỗi target.

### 2b. CORE vs OPAQUE (chốt sau khi đọc ast.rs — 40+ Node, đuôi exotic dài)

AST có cái đuôi KHÔNG fit 3-address: `Sync` (atomics LL/SC), `Overflow`, `Asm`
(inline), `VaStart/VaArg/VaArea` (va_list AAPCS), `Alloca`/VLA, `SRet` (struct
return), `Tramp/Upvar/NlGoto` (nested fn), TLS. Ép chúng vào 3-address = phình +
rủi ro vô ích.

→ **IR chia 2 hạng inst:**
- **CORE** (Bin/Un/Load/Store/Lea/Cast/Call/Copy + terminator): typed 3-address,
  interp evaluate được, verifier phủ, **pass CHỈ đụng hạng này**.
- **OPAQUE** (`Inst::Op(...)` bọc nguyên construct AST exotic): hạ 1-1 xuống
  backend Y NHƯ codegen hiện tại xử — pass KHÔNG touch, interp gọi ra một
  handler riêng (hoặc đánh dấu "impure, không fold qua"). Correctness cái đuôi =
  nguyên trạng, không refactor.

Nhờ vậy: (a) IR core tối giản, (b) opt chỉ chơi trên core (an toàn), (c) migrate
AST→IR không phải viết lại logic exotic — chỉ bọc lại.

## 3. Contract = 3 tài liệu bất biến (đây là "chuẩn IR đỡ bug")

Không mượn format ngoài (QBE/LLVM) làm dependency — zero-crate + dựng từ định lý.
Học TRIẾT LÝ QBE (typed, minimal, spec ngắn). "Chuẩn đỡ bug" đến từ 3 thứ hình thức:

### 3a. Verifier — automaton kiểm well-formedness (chạy SAU mỗi pass)
Bất biến (reject nếu vỡ, không để trôi xuống asm rác):
- **Typed**: mọi Temp gán đúng type; op có type-signature khớp toán hạng.
- **Def-before-use** trên toàn CFG (mọi path tới use đều qua def).
- **CFG well-formed**: mỗi block kết bằng đúng 1 terminator; target block tồn tại;
  entry không có predecessor rẽ vào giữa.
- **No dangling**: Temp/Block/global ref hợp lệ.

### 3b. Interp — reference evaluator (ground truth ngữ nghĩa)
Chạy IR trực tiếp (không qua asm) → kết quả quan sát được. Dùng làm oracle NỘI TẠI.
**Ngữ nghĩa hình thức hoá đầy đủ (NẤC-1): `SEMANTICS.md`** — trạng thái Σ=⟨ρ,μ⟩ +
định nghĩa toán học ⟦·⟧ mỗi Inst/Term, map 1-1 với `ir.rs::tests::interp`:

### 3c. Commuting square — mỗi pass phải GIAO HOÁN với interp
```
   ir_before ──interp──▶ result
      │                    ‖
    pass                   ‖   (BẮT BUỘC bằng)
      ▼                    ‖
   ir_after  ──interp──▶ result
```
Pass đúng ⟺ giao hoán với interp. Đây là commuting-square fold↔runtime (đã có ở
`alg.sh`) nâng lên tầng IR — bắt bug NGAY tại pass đẻ ra nó, không chờ end-to-end.
**Nâng thành ĐỊNH LÝ EXECUTABLE (NẤC-1, xong):** `opt.rs::commuting_square_
structural_exhaustion` vét cạn `𝔼_struct` (312 biểu thức (5 họ shape) × 5 pass = 1560 ô) chứng
∀e giao hoán, + `commuting_square_selfproof` (anti-blindness). Phát biểu: SEMANTICS §5.

## 4. Lowering (nơi correctness sống)

- **AST → IR** (`ir.rs`): dịch per-node cây AST phẳng thành block+inst. Đây +
  IR→asm là HAI chỗ duy nhất giữ correctness. UAC/cast/va/HFA… hạ ở đây (parser
  đã chèn Node::Cast → 1-1). Differential: interp(IR) khớp chạy asm hiện tại.
- **IR → asm** (mỗi target): per-inst instruction-selection + regalloc + ABI.
  ABI automaton (arg offset 3-nơi-khớp) chuyển TRỌN về đây — frontend hết dính.

## 5. Pass layer (TƯƠNG LAI — NGOÀI baseline, Vu defer opt)

> Baseline KHÔNG gõ mục này. Giữ làm bản thiết kế cho lúc mở opt. Bất biến §5 bên
> dưới là điều kiện an toàn để sau này bật từng pass.

(LỚP PHỦ tắt được — 70-90% rủi ro nhốt ở đây)

Bất biến an toàn: **IR lower thẳng ra asm ĐÚNG mà không cần pass nào**. Tắt sạch
pass → suite vẫn xanh (chỉ chậm/nhiều stack). Proof-by-deletion như ext.rs.

Thứ tự = rủi ro TĂNG DẦN, mỗi pass đóng sổ (verifier + commuting-square) mới sang:
1. **regalloc** (gần bắt buộc — naive stack-slot đúng nhưng thảm): linear-scan
   trên live-interval. Định lý: interference graph / liveness dataflow.
2. **const-fold / peephole** (local, verify dễ): abstract-interpretation trên
   lattice hằng.
3. **DCE** (cần liveness GLOBAL, rủi ro hơn): định lý reachability.

Không thêm pass "vì đẹp". Pass nào không phát biểu được §6 → không gõ.

## 6. Luật "mỗi LOC một định lý" — cụ thể hoá

Một pass CHỈ được viết khi phát biểu đủ 4 phần:
1. **Input invariant** (IR well-formed trước pass).
2. **Rewrite rule** (biến đổi làm gì).
3. **Preservation theorem** (vì sao output ≡ input về observable behavior).
4. **Output invariant** (verifier vẫn pass sau).

UB filter là luật gốc: optimizer KHAI THÁC UB → generator lọc UB TRƯỚC, differential
tại UB vô nghĩa gấp đôi khi có opt.

## 7. Kế hoạch triển khai (mỗi bước đóng sổ xanh trước khi bước tiếp)

1. **ir.rs skeleton**: IR types + verifier + interp (interp/verifier ở
   `#[cfg(test)]` hoặc sau cờ debug ⇒ test-side, không tính trần). Chưa nối
   codegen. Test: interp chạy vài IR viết tay (factorial, branch, load/store)
   khớp kỳ vọng. ← ĐANG Ở ĐÂY.
2. **AST → IR lowering** đủ phủ corpus hiện hành. Codegen ELF đọc IR thay AST.
   CỔNG: 14/14 suite xanh qua đường IR (differential với baseline asm cũ).
3. **Xoá đường AST→asm cũ** (arm64_elf.rs thành IR→asm thuần). Đo LOC — phải ≤10k.
4. **DỪNG baseline ở đây** (Vu defer opt). Pass layer §5 chỉ mở khi Vu bật.

## 8. Sổ rủi ro / gate

- Nghi "pass đúng" → commuting-square oracle phải in verdict TRƯỚC khi tuyên (luật
  đo-trước-khi-nói).
- Sci-gate mở rộng: thêm `ir.sh` (verifier vét well-formedness + interp↔asm
  differential trên không gian IR sinh máy).
- Mọi verdict xanh kèm evidence trail cơ học (số IR-func verify + số commuting
  point khớp), không chỉ pass/fail.

---
Điểm cần Vu chốt trước khi gõ §7.1: (a) IR có tách file `ir.rs` riêng hay gộp
`ast.rs`? (đề xuất: file riêng, boundary sạch). (b) interp trả gì làm "observable"
— exit code + memory-trace, hay chỉ giá trị return? (đề xuất: exit + syscall-trace
tối giản để so I/O). (c) regalloc linear-scan hay graph-coloring? (đề xuất:
linear-scan, ít LOC + đủ −O0).
