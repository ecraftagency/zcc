# zcc IR — Reference Semantics ⟦·⟧ (NẤC-1, mechanized reference semantics)

> **Trạng thái (thành thật, luật suy-đoán-tội áp vào cả doc):** đây là **mechanized
> reference semantics** — ngữ nghĩa tham chiếu hình thức hoá, HIỆN THỰC bằng
> `src/ir.rs::interp` (test-side), và ĐƯỢC KIỂM bằng vét-cạn-cấu-trúc (`opt.rs::
> commuting_square_structural_exhaustion`). **KHÔNG phải machine-checked proof.**
> Cấm dán nhãn "verified"/"CompCert-level". Rice's theorem chặn tương đương phổ
> quát ⟹ mọi định lý dưới đây phát biểu trên LỚP HÌNH DẠNG hữu hạn (decidable), và
> được ĐO cơ học, không CHỨNG phổ quát. Đây là NỀN (nấc-1) cho translation-
> validation (nấc-2) và per-pass machine-check (nấc-3) — xem memory
> `zcc-formal-verification-roadmap`.

Tài liệu này là **định nghĩa toán học của mỗi `Inst`** (yêu cầu NẤC-1). Nó là
*spec của code*, không phải nguyện vọng: mỗi rule map 1-1 tới một arm trong
`ir.rs::tests::interp` / một hàm nghĩa nguyên tử (`eval_bin`/`eval_cast`/`canon`)
đã sống ở phần **non-test** của `ir.rs` (dùng CHUNG với const-fold ⟹ faithfulness).

Liên quan: `IR.md` §3b/§3c (contract), `THEORY.md` §A7 (denotational semantics),
`tests/alg.sh` (commuting-square fold↔runtime tầng SOURCE — bản này nâng lên IR).

---

## 1. Miền giá trị (value domain)

Một **machine value** `Val` là một từ 64-bit *canonical*, khớp hợp đồng "canonical
register" của `ast.rs`:

```
𝕍 = { canon_τ(z) : z ∈ ℤ }              cho kiểu nguyên τ (bề rộng w=size(τ)·8, dấu s)
   ∪ { bits(x) : x ∈ 𝔽₆₄ }             cho float (LƯU BIT-PATTERN f64; float 32-bit nâng lên f64)
```

- **Số nguyên:** giá trị mang trong ℤ/2^w với dấu s. `canon_τ` (⇐ `ir.rs::canon`)
  chuẩn hoá `z ∈ ℤ` về đại diện canonical: mask w bit thấp rồi sign-extend nếu s.
  Đây CHÍNH là `ext(ct)` của backend — "số học int wrap tại w bit" (THEORY §B5,
  two's-complement = modulo 2^w).
- **Float:** giá trị là bit-pattern IEEE-754 f64. Float C 32-bit (`float`) nâng lên
  f64 khi nạp register (`Load` sz=4 → `f32→f64`), hạ lại khi ghi (`Store` sz=4).
  Bit-pattern giữ nguyên (không reassociate — FP KHÔNG kết hợp, THEORY §B5).

`TypeId τ` mang **cấu trúc đại số**: Op là ký hiệu thuần; ℤ/2^w (int, có dấu) hay ℝ
(≈ f64, float) do τ quyết định. Tách "phép" khỏi "cấu trúc" (ir.rs::Op doc).

---

## 2. Trạng thái máy (machine state) Σ

```
Σ  =  ⟨ ρ , μ ⟩
ρ  :  Tmp → 𝕍            register file (bảng temp; ρ[t] = giá trị canonical của t)
μ  :  [0, frame) → Byte  bộ nhớ local phẳng (mảng byte cỡ frame khung stack)
```

**Mô hình bộ nhớ (⇐ interp):** chỉ local frame được mô hình hoá. Địa chỉ local =
`x29 − off`; flat-mem index 0 ⟺ `x29 − frame` ⟹ `index(off) = frame − off`
(`Lea Local`). `load_mem`/`store_mem` = little-endian byte-serialize (LP64 Darwin/
AArch64-LE). **Global/Str KHÔNG mô hình hoá** → ⊥ (§4): hàm chạm chúng nằm NGOÀI
không gian CORE.

**Seed tham số:** param `(off, τ)` thứ i ← `canon_τ(argᵢ)` ghi vào `μ` tại
`index(off)`, width `size(τ)`. KHÔNG có param-temp: body đọc mọi biến (kể cả param)
qua `Var→Load` (mô hình -O0 nhất quán, IR.md §4).

**Observable:** giá trị TRẢ (return value) — `⟦Func⟧(args) ∈ 𝕍`. (Trace I/O bỏ:
CORE thuần tính toán; hàm có side-effect ngoài (Call extern, Asm) = exotic → ⊥.)

---

## 3. Hàm nghĩa nguyên tử (atomic denotations) — KEYSTONE faithfulness

Ba hàm sau sống ở phần **non-test** `ir.rs` và ĐƯỢC GỌI BỞI CẢ interp (proof-side)
LẪN const-fold (`opt.rs`, release). MỘT định nghĩa ⟹ folder và interpreter KHÔNG
THỂ lệch: `⟦fold(e)⟧ = ⟦e⟧` đúng **by construction** (THEORY §A7 term-rewriting
soundness, §III keystone).

### 3.1 `canon_τ : ℤ → 𝕍`  (`ir.rs::canon`)
```
canon_τ(v) = v                                    nếu float(τ) ∨ size(τ) ≥ 8
           = sext_w( v mod 2^w )                   nếu int có dấu,   w = size(τ)·8
           = ( v mod 2^w )                         nếu int không dấu
```

### 3.2 `⟦op⟧_τ : 𝕍 × 𝕍 → 𝕍 ∪ {⊥}`  (`ir.rs::eval_bin`)
- **float(τ):** giải bit→f64, áp op ∈ {+,−,×,÷} trong 𝔽₆₄ (IEEE-754), so sánh →
  {0,1}. (÷ float KHÔNG ⊥: theo IEEE cho ±∞/NaN.)
- **int(τ):** số học trong ℤ/2^w với dấu s, kết quả `canon_τ`:
  - `+,−,×` = wrapping (modulo 2^w).
  - `÷,%` : **`y=0 → ⊥`** (UB — const-fold PHẢI bỏ qua, giữ lệnh cho runtime).
    Ngược lại chia cắt-về-0 (signed `wrapping_div`, unsigned `u64`).
  - `& | ^` bitwise; `<<` = wrapping_shl; `>>` = arith (signed) / logic (unsigned).
  - `== != < ≤ > ≥` → {0,1}, so sánh có dấu theo s.

### 3.3 `⟦cast⟧_{σ→τ} : 𝕍 → 𝕍`  (`ir.rs::eval_cast`)  (C99 6.3.1.2 / 6.3.1.4)
```
int→int   : _Bool ⟹ (v≠0);        ngược lại canon_τ(v)         (trunc/ext)
int→float : (float)v  (unsigned dùng u64→f64)
float→int : _Bool ⟹ (f≠0);        ngược lại canon_τ(⌊f⌋)       (trunc-về-0)
float→float: v                     (f64 canonical cả hai)
```

---

## 4. Ngữ nghĩa lệnh ⟦Inst⟧ : Σ → Σ  (CORE — big-step, ⇐ interp `match inst`)

Ký hiệu `⟨v⟩ρ` = fetch: `Tmp t↦ρ[t]`, `Imm x↦x`, `FImm b↦b`. `ρ[d↦u]` = cập nhật.

| Inst | ⟦·⟧ : Σ → Σ (rule) |
|---|---|
| `Bin(d,op,τ,a,b)` | `ρ' = ρ[d ↦ ⟦op⟧_τ(⟨a⟩ρ, ⟨b⟩ρ)]`   (⊥ nếu op ⊥) |
| `Un(d,⊝,τ,a)` | `ρ[d ↦ canon_τ(−⟨a⟩)]` (Neg int) / `bits(−f)` (Neg float) / `canon_τ(¬⟨a⟩)` (BNot) |
| `Copy(d,τ,a)` | `ρ[d ↦ canon_τ(⟨a⟩ρ)]` |
| `Load(d,τ,a)` | `ρ[d ↦ decode_τ(μ, ⟨a⟩ρ)]`  (đọc size(τ) byte; f32→f64 nếu float sz=4) |
| `Store(τ,a,v)` | `μ' = μ[⟨a⟩ρ ↦ encode_τ(⟨v⟩ρ)]`  (ghi size(τ) byte; f64→f32 nếu sz=4) |
| `Memcpy(d,s,n)` | `μ' = μ[⟨d⟩ρ ..+n ↦ μ(⟨s⟩ρ ..+n)]`  (copy n byte xuôi — struct-assign C99 6.5.16) |
| `Lea(d, Local off)` | `ρ[d ↦ frame − off]`   (`Global`/`Str` → ⊥) |
| `Cast(d,σ,τ,a)` | `ρ[d ↦ ⟦cast⟧_{σ→τ}(⟨a⟩ρ)]` |
| `Call(Some d, Sym g, ā, _)` | `ρ[d ↦ canon_{τd}( ⟦g⟧(⟨ā⟩ρ) )]`  (đệ quy big-step; `Ptr`/depth>500 → ⊥) |

**EXOTIC (⊥ — impure, NGOÀI không gian CORE):** `FunAddr, LabelAddr, Zero, VaStart,
VaArg, Overflow, VaArea, GotoPtr, Alloca, CallX, Sync, Asm`. Interp trả `Err` ⟹
input rơi vào hàm chứa exotic = "không thuần" ⟹ commuting-square SKIP (giống UB).
Đây là hàng rào CORE/EXOTIC-typed của IR.md §2b: pass CHỈ đụng CORE, nên chỉ cần
⟦·⟧ trên CORE để chứng pass giao hoán.

## 4b. Ngữ nghĩa terminator ⟦Term⟧ : Σ → (BlockId ⊎ Halt)  (⇐ interp `match term`)

```
⟦Jmp b⟧          =  goto b
⟦Br c b_t b_e⟧   =  goto (⟨c⟩ρ ≠ 0 ? b_t : b_e)
⟦Ret v?⟧         =  Halt(⟨v⟩ρ)   (Halt(0) nếu None)
⟦Unreachable⟧    =  ⊥            (chạm = IR hỏng hoặc dead-code thật sự bất khả đạt)
```

## 4c. Big-step hàm ⟦Func⟧ : 𝕍* → 𝕍 ∪ {⊥}

```
⟦f⟧(ā) = eval từ block 0 với Σ₀ = ⟨ρ=0̄, μ=seed(ā)⟩; chạy ⟦inst⟧ tuần tự trong
         block, rồi ⟦term⟧ chuyển block; dừng tại Halt(v) ⟹ v. Chốt an toàn:
         budget bước (lặp vô hạn → ⊥) + depth Call ≤ 500 (đệ quy host → ⊥).
```
`⊥` (Err) = "input NGOÀI không gian mô hình" (UB div0, exotic, global, đệ quy sâu,
lặp quá ngân sách). Diff-tại-⊥ vô nghĩa (luật gốc) ⟹ commuting-square bỏ qua.

---

## 5. ĐỊNH LÝ commuting-square (executable) — nâng từ `alg.sh`

`alg.sh` chứng giao hoán fold↔runtime ở tầng **SOURCE** (diff hai binary sinh bởi
cc/zcc trên không gian đại số vét cạn). NẤC-1 nâng lên tầng **IR + reference
semantics** (in-process, zero-dep, KHÔNG cần cc):

> **Định lý (metamorphic / translation-validation vét-cạn-cấu-trúc).**
> Với mọi pass `P ∈ {const_fold, copy_prop, cse, dce, optimize}`, mọi biểu thức
> `e` trong không gian sinh có cấu trúc `𝔼_struct`, và mọi input `i ∈ battery`:
> $$ ⟦lower(e)⟧(i) \ne ⊥ \;\Longrightarrow\; ⟦P(lower(e))⟧(i) = ⟦lower(e)⟧(i). $$

Nói cách khác biểu đồ sau GIAO HOÁN với mọi `e ∈ 𝔼_struct`:

```
      lower(e) ───────⟦·⟧──────▶  v
         │                        ‖
         P                        ‖   (BẮT BUỘC bằng, ∀ i mà ⟦·⟧≠⊥)
         ▼                        ‖
     P(lower(e)) ────⟦·⟧──────▶  v
```

**Kiểm cơ học:** `opt.rs::tests::commuting_square_structural_exhaustion`.
`𝔼_struct` = hợp **năm HỌ shape**, mỗi họ vét cạn op trên một cấu trúc riêng ⟹ phủ
mọi loại Inst (Bin/Un/Copy/Load/Store/Lea/Cast) + cả hai loại Term (Jmp/Br):

| họ | shape | cỡ | kích pass/Inst |
|---|---|---|---|
| A | arith straight-line (`POOL³`) | 216 | fold+CSE+copy+DCE, Bin |
| B | div/mod (`POOL×{/,%}`) | 12 | UB-skip đối xứng, fold-từ-chối-div0 |
| C | shift (`POOL×{<<,>>}`) | 12 | Shl/Shr (>> xét dấu) |
| D | con trỏ/bộ nhớ (`POOL²`) | 36 | Lea/Load/Store, **memory-kill CSE** (pr84169) |
| E | vòng lặp/CFG (`POOL²`) | 36 | Br/Jmp back-edge, copy-prop/DCE qua biên block |

Tổng **312 biểu thức** × 5 pass = **1560 ô commuting-square** đóng xanh. `⟦·⟧` =
`interp`; equiv so trên `battery` (vét-cạn-miền-nhỏ [−6,6]ⁿ + biên INT_MAX/MIN,
ir.rs::battery). Họ E dùng trip-count `b&7` ⟹ interp LUÔN dừng (non-vacuous).
Evidence trail cơ học (số expr + số ô) assert cứng — cấm "pass rỗng" (luật input-sạch).

**Anti-blindness:** `commuting_square_selfproof` đột biến (xoá một `Store` → mất
ghi-mem) và đòi commuting-square PHẢI bắt — nếu equiv mù thì mọi verdict vô giá
trị (tự-chứng công cụ chứng).

---

## 6. Giới hạn (thành thật) & lối lên nấc trên

- ⟦·⟧ mô hình **local memory + return value**; KHÔNG mô hình global/heap/I/O/
  concurrency ⟹ chỉ chứng được pass trên hàm CORE-thuần. Đủ cho 5 pass hiện tại
  (đều CORE), KHÔNG đủ cho tối ưu liên-thủ-tục / alias toàn cục.
- Vét-cạn `𝔼_struct` là **cấu trúc hữu hạn**, KHÔNG phổ quát: bắt bug cấu trúc-đã-
  sinh, KHÔNG chứng ∀ chương trình (Rice). Để lên **nấc-2** (translation validation
  Tristan–Leroy: certificate + checker per-compilation) và **nấc-3** (machine-check
  từng pass trong Coq/HOL4), reference semantics §1–§4 này là ĐỐI TƯỢNG được
  formal hoá — đó là lý do nó phải phát biểu tường minh, map 1-1 với code, ở đây.
