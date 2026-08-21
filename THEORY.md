# THEORY.md — nền lý thuyết của zcc

> Hiến chương (CLAUDE.md) luật gốc **MATHEMATIC FOUNDATION**. File này là catalog
> **tất tần tật, đầy đủ** — hỏi "zcc dựa trên nền lý thuyết nào" → in file này.
> Cập nhật mỗi khi thêm định lý/hằng/bảng mới.

---

## §0. LUẬT GỐC — định lý phân rã source code (nếu chỉ giữ 1 luật, giữ luật này)

```
zcc source  =  ( math / theory     → control-flow + data-structure + algorithm )
            ⊕  ( iso / os / arch / gcc spec → constant + param + value-table )
```

**Mỗi dòng `src/` thuộc ĐÚNG một trong hai vế. Không có dòng thứ ba.**

- **Vế I (theory → cách TÍNH):** luồng điều khiển, cấu trúc dữ liệu, thuật toán —
  đều rút ra từ một định lý / cấu trúc toán. Nếu một đoạn code không map được về
  định lý nào ở Phần I → nghi sai kiến trúc.
- **Vế II (spec → GIÁ TRỊ):** hằng số, tham số, bảng tra — chép từ văn bản chuẩn
  (ISO C99 / AAPCS64 / System V / ELF / AArch64 ARM ARM / GNU). Không "magic number
  không nguồn gốc" (luật số-liệu-của-Vu): mọi hằng phải truy được về một dòng spec.

Hệ quả kiểm tra: `grep 'EXT(' src/` phủ 100% bề mặt lệch chuẩn (vế II, nhánh gcc/apple);
mọi hằng layout/ABI sống trong TyTab + file target (vế II); phần còn lại là vế I.

---

## §0b. CORRECTNESS LÀ GÌ — real-software coverage là bằng chứng RẺ

- Một compiler **10–15k LOC** có thể compile **250+ phần mềm thật** một cách dễ dàng —
  vì C bảo thủ chỉ dùng một **tập con hẹp & chung** của ngôn ngữ. Phủ được nhiều
  project ⟹ chứng minh **tính khả dụng**, KHÔNG chứng minh **tính đúng**.
- Ngược lại: **chục compiler cùng cỡ vẫn TRƯỢT csmith/yarpgen** — random differential
  tra tấn đúng các góc ngữ nghĩa mà phần mềm thật không bao giờ chạm (thứ tự eval,
  UAC biên, bitfield packing, sign/overflow, alias, ABI hiếm).
- ⟹ **Thang bằng chứng correctness (yếu → mạnh):**
  `compile-được-app  <  chạy-đúng-app  <  differential-vs-oracle trên corpus  <  structural-exhaustion (sci-gate)  <  random-differential (csmith/yarpgen)  <  IR-equivalence-by-theorem`.
- ⟹ Lý do tồn tại của **tầng sci-gate** (vét cạn cấu trúc, ground truth) và của hướng
  **IR→IR_ops chứng bằng định lý**: chúng bắt lỗi mà 250 app không bao giờ lộ.
  App-stack là chứng nghiệm THỰC TIỄN (tầng dưới); định lý là ground truth (tầng trên).

---

# PHẦN I — THEORY → CONTROL-FLOW / DATA-STRUCTURE / ALGORITHM

> Vế I của §0: *cách zcc tính*. Tra theo 4 trục: **A** pha pipeline · **B** ngành toán
> thuần · **C** computability/complexity · **D** sci-gate. Trạng thái: **[DÙNG]** đã
> hiện thực+gate · **[SẼ DÙNG]** IR/opt tier · **[NỀN]** ngầm (mọi quyết định tựa vào).

## A — THEO PHA PIPELINE

### A1. Lexing `[DÙNG]`
| khái niệm/định lý | mô tả | zcc |
|---|---|---|
| Regular language | token = ngôn ngữ chính quy | `lexer.rs`, `gate shape` |
| Finite automaton (DFA/NFA), Kleene | máy trạng thái hữu hạn; RE↔DFA | `lexer.rs` |
| Maximal munch / longest-match | token dài nhất (`>>`,`->`) | `lexer.rs` |
| Chomsky hierarchy (Type-3 ⊂ Type-2) | token chính quy ⊂ CFG parser | `lexer.rs`↔`parser.rs` |
| Translation phases (8 pha, 5.1.1.2) | splice `\`, comment, token, macro | `lexer.rs`+`preprocess.rs` |

### A2. Preprocessing `[DÙNG]`
| khái niệm/định lý | mô tả | zcc |
|---|---|---|
| Term rewriting system (TRS) | macro expand = viết lại hạng → normal form | `preprocess.rs`, `gate cpp` |
| Confluence (Church–Rosser) | kết quả expand xác định | `preprocess.rs` |
| Termination / well-foundedness | expansion phải dừng | `preprocess.rs` |
| Hideset / blue paint | chống đệ quy macro | `preprocess.rs` |
| Constant-expression eval (#if) | eval hằng nguyên (grammar con+interp) | `gate cpp` |

### A3. Parsing `[DÙNG]`
| khái niệm/định lý | mô tả | zcc |
|---|---|---|
| Context-free grammar (Type-2) | ngữ pháp C = CFG | `parser.rs`, `gate shape` |
| Recursive descent (LL, top-down) | đệ quy xuống theo luật | `parser.rs` |
| Precedence climbing / Pratt | leo ưu tiên toán tử nhị phân | `parser.rs` (`mkbin`+bp) |
| Lexer hack (typedef feedback) | `T*x` decl vs mul cần bảng typedef | `parser.rs` (`is_type_word`) |
| Dangling-else resolution | `else` khớp `if` gần nhất | `parser.rs` |
| Inductive datatype / term algebra | AST = arena + `NodeId(u32)`, không Box | `ast.rs` |

### A4. Type system & static semantics `[DÙNG]`
| khái niệm/định lý | mô tả | zcc |
|---|---|---|
| Type-derivation lattice | dẫn xuất ptr/array/func; decay array→ptr | `gate decay` |
| UAC = join-semilattice | least-upper-bound trên rank | `parser.rs` (`common_ty`), `gate alg` |
| Integer promotion / rank order (6.3.1.1) | thứ tự trên rank | `parser.rs` (`promote`) |
| Typing judgment Γ⊢e:τ | môi trường kiểu + scope/shadowing | `parser.rs` (`locals`,`typedefs`) |
| Record-layout automaton | struct/union/bitfield = cursor có trạng thái | `gate shape` |
| Constant folding = partial evaluation | eval hằng lúc dịch | `parser.rs` (`fold`), `gate alg` |
| Commuting square fold↔runtime | fold(e)=run(e) | `gate alg` |

### A5. Codegen & ABI `[DÙNG]`
| khái niệm/định lý | mô tả | zcc |
|---|---|---|
| Instruction selection = per-node simulation | mô phỏng node AST→asm (maximal munch trên cây) | `codegen/arm64_elf.rs` |
| ABI = finite automaton | phân loại arg (NGRN/NSRN/NSAA) | `gate abi` |
| Cross-link cancellation | lỗi ABI cùng-compiler tự khử → gate 4 hướng | `gate abi` |
| Activation record / frame layout | fp-relative, spill, variadic save | `codegen/arm64_elf.rs` |

### A6. IR — dạng trung gian `[SẼ DÙNG, khung đã dựng]`
| khái niệm/định lý | mô tả | zcc |
|---|---|---|
| Control-flow graph (CFG) | hàm = đồ thị block | `ir.rs` (`IrFunc.blocks`) |
| Basic block | chuỗi thẳng + đúng 1 terminator | `ir.rs` (`Block`) |
| Terminator = automaton trên BlockId | Jmp/Br/Ret/Switch/Unreachable | `ir.rs` (`Term`) |
| Virtual registers / temps (SSA-free) | temp có kiểu Γ | `ir.rs` (`temps`) |
| CORE vs EXOTIC-typed two-tier | CORE (Bin/Un/Copy/Load/Lea/Cast) opt đụng được; EXOTIC-typed (Call/Store/Overflow/Va*/Sync/Asm…) impure, không DCE/CSE (Inst::Opaque đã XÓA) | `ir.rs` (`Inst`) |
| Well-formedness verifier | ref-integrity+def-coverage+entry | `ir.rs` (`verify`) |
| SSA + φ-node *(mở khi cần)* | mỗi temp gán 1 lần | *[chưa]* |

### A7. Optimization — chứng minh pass `[DÙNG: IR→IR proven]` (mỗi pass provable, thay trần LOC)

> Hiện trạng 2026-08-21: 5 pass đã hiện thực + CHỨNG ở IR→IR (`src/opt.rs`, 29 test):
> const-fold / DCE / copy-prop / CSE gate bằng `ir::tests::equiv` (commuting-square,
> ⟦A⟧≡⟦P(A)⟧ trên battery vét-cạn-miền-nhỏ + biên) + `verify`; regalloc (liveness
> dataflow-fixpoint → interference → Chaitin coloring → `verify_coloring`) gate bằng
> BẤT BIẾN GIAO THOA (bisimulation-đổi-tên). Orchestrator `optimize()` = fixpoint 1-4,
> chạy sau cờ `ZCC_OPT` trên đường IR mặc định (đo box: torture opt≡noopt end-to-end).
| khái niệm/định lý | mô tả |
|---|---|
| Denotational semantics ⟦·⟧:State→State | pass đúng ⟺ ⟦f⟧=⟦f'⟧ |
| Operational semantics (small/big-step) | interp = hiện thực ⟦·⟧ |
| Translation validation (Pnueli/Necula) | validate MỖI LẦN chạy pass |
| Bisimulation / simulation | khớp trạng thái edge-by-edge (regalloc) |
| Symbolic execution | biến ký hiệu→term đóng; COMPLETE loop-free |
| Value numbering / congruence / e-graph | chuẩn hoá + nền CSE |
| Term-rewriting soundness ⟦L⟧=⟦R⟧ | correctness BY CONSTRUCTION |
| Newman's lemma | terminating+local-confluent → confluent |
| Dataflow = monotone framework trên lattice | leo fixpoint |
| Fixpoint Kleene / Knaster–Tarski | least/greatest fixpoint |
| Liveness / reaching-defs / available-expr | nền DCE/copy-prop/CSE |
| Dominance / dom-tree (Lengauer–Tarski) | A dom B; nền copy-prop, SSA |
| Graph coloring / interference (Chaitin–Briggs) | regalloc = tô màu |

**5 pass → định lý (đều DECIDABLE, không tái-cấu-trúc loop → thoát Rice):**
const-fold=rewrite-soundness · DCE=liveness · copy-prop=dominance+Leibniz ·
CSE=value-numbering · regalloc=bisimulation-đổi-tên.

### A8. Testing & proof methodology `[DÙNG]`
Differential testing · Metamorphic (commuting-square) · Property/boundary-value ·
Structural exhaustion · UB filtering · 2-fact (PASS|NOT-IMPL|FAIL, gate=0 FAIL) ·
Translation-validation-as-gate (`ir.sh`, kế hoạch) · Evidence-trail (input-sạch).

## B — THEO NGÀNH TOÁN THUẦN (index ngược)

- **B1. Rời rạc & đồ thị:** automata/formal-lang (A1–A3), đồ thị có hướng (CFG, dom-tree,
  interference), cây (AST, expr-tree), tổ hợp/đếm (generator vét cạn), quan hệ tương đương
  (bisimulation, value-number classes). Thuật: DFS/postorder, reverse-postorder, SCC.
- **B2. Đại số:** semilattice (UAC join, dataflow meet), lattice/complete-lattice (type,
  dataflow; nền Tarski), term-algebra tự do (AST/IR), monoid/associativity (nối token/block/fold),
  Boolean algebra (`#if`, branch, bit-ops), đại số tuyến tính thưa (offset/stride mảng nhiều
  chiều = ánh xạ affine index→address, VLA-2D `i·rowsz+j·esz`).
- **B3. Thứ tự (order):** poset (rank, dominance, lattice), monotone+fixpoint (Kleene chain,
  Knaster–Tarski), well-founded/termination (macro, rewriting), Galois connection [NỀN, abstract-interp].
- **B4. Logic & proof theory:** typing judgment/natural-deduction (Γ⊢), Hoare logic/wp [SẼ DÙNG],
  FOL/SMT-style (symbolic path condition, decidable loop-free) [SẼ DÙNG], Leibniz equality (copy-prop),
  SAT [NỀN].
- **B5. Giải tích & số học máy (vai trò HẸP nhưng thật):** IEEE-754 FP semantics (rounding/NaN/Inf/
  signed-zero — codegen giữ bit-pattern), real-analysis [NỀN] (FP KHÔNG kết hợp → cấm reorder fold
  float), monotone convergence (dataflow đạt fixpoint hữu hạn), number-theory/modular (align=modulo
  2^k, two's-complement=modulo 2^n, `%`/`/` truncation-toward-zero C99).
- **B6. Xác suất (phương pháp test):** random differential / fuzzing (csmith/yarpgen, kế hoạch) —
  kỳ vọng phủ lỗi ∝ số mẫu, dưới structural-exhaustion về độ chắc (xem §0b).

## C — COMPUTABILITY & COMPLEXITY (arch complexity)

**C1. Computability:** Halting problem/undecidability (gốc mọi giới hạn) · **Rice's theorem**
(⟦f⟧=⟦f'⟧ tổng quát bất khả quyết → pass phải ràng-buộc-hình-dạng vào lớp decidable) ·
decidable fragment (loop-free/bounded → symbolic-equiv COMPLETE) · recursively-enumerable (tập chương trình hợp-lệ).

**C2. Complexity từng phase:** lexing **O(n)** · preprocess **O(n)** amortized (hideset chặn blow-up) ·
parse recursive-descent **O(n)** (không backtrack lũy thừa) · type/layout **O(n)** · codegen **O(n)** ·
dataflow **O(n·h·|lattice|)** · dom-tree **O(n·α(n))** Lengauer–Tarski · value-numbering **O(n)–O(n log n)** ·
**register allocation NP-complete** (Chaitin) → heuristic simplify/spill · SSA construction **O(n·α(n))**.

**C3. Lớp phức tạp:** P (frontend + hầu hết analysis) · NP-complete (regalloc — vì sao heuristic,
không "tối ưu tuyệt đối"; nhưng *đúng-màu* verify được trong P) · undecidable (equivalence tổng quát
→ chỉ structural + translation-validation per-run) · **complexity của CHÍNH compiler** (bất biến `src/`
≤ trần: compiler=định lý phải đọc được).

## D — SCI-GATE ↔ ĐỊNH LÝ (ground truth tier)
| gate | không gian vét cạn | định lý |
|---|---|---|
| `shape` | lexer/declarator/layout | grammar automata + record-layout automaton |
| `cpp` | preprocessor | term rewriting system + #if const-eval |
| `decay` | dẫn xuất kiểu | type-derivation lattice |
| `alg` | UAC + fold | join-semilattice + commuting-square fold↔runtime |
| `abi` | ABI classify + link | finite automaton + cross-link cancellation |
| `ir` *(`cargo test opt::`)* | IR + 5 pass | denotation preservation: equiv commuting-square (fold/DCE/copy/CSE) + interference-invariant (regalloc) |

---

# PHẦN II — SPEC → CONSTANT / PARAM / VALUE-TABLE

> Vế II của §0: *giá trị zcc chép từ chuẩn*. Mọi hằng phải truy được về một dòng spec
> (luật số-liệu-của-Vu). Nơi sống: **TyTab trong `ast.rs`** (layout, LP64) + **file target**
> (ABI/section/asm) + **`ext.rs` + marker `EXT(...)`** (bề mặt vendor). Target: AArch64 ELF Linux.

### II-1. ISO C99 — hằng ngôn ngữ
| bảng/hằng | nguồn spec | zcc |
|---|---|---|
| integer conversion rank | 6.3.1.1 | `parser.rs` promote/common_ty |
| `<limits.h>` (INT_MAX, CHAR_BIT=8…) | 5.2.4.2.1 | header + TyTab size |
| UAC bảng chuyển | 6.3.1.8 | `common_ty` |
| escape/trigraph, số literal suffix | 6.4.4 | `lexer.rs` |
| source/exec char set = UTF-8 multibyte (decode-table RFC 3629: mask `0x1f/0x0f/0x07/0x3f`, shift 6) | 5.1.1.2 + 6.4.5 | `lexer.rs` `utf8_cp` |
| `%`,`/` truncation-toward-zero; overflow signed = UB | 6.5.5 | codegen + UB-filter |
| char = **unsigned** (AAPCS64 aarch64 default, khóa) | 6.2.5 + AAPCS64 | TyTab (`char`→UCHAR) |

### II-2. Memory model — kích thước & alignment (LP64, khóa)
| kiểu | size | align | nguồn |
|---|---|---|---|
| char/short/int/long/long long | 1/2/4/8/8 | =size | LP64 (System V AArch64) |
| pointer | 8 | 8 | LP64 |
| float/double | 4/8 | =size | LP64 |
| long double | **16** | **16** | binary128 memory/ABI (AAPCS64); *số học* làm như double (float.h `LDBL_MANT_DIG=53`), libgcc `__extenddftf2`/`__trunctfdf2` ở biên — lựa-chọn có sổ |
| struct/union | Σ có padding | max field, aggregate ≥ **8** cho `data_align` | AAPCS64 §5.1 |
| bitfield | packing theo storage-unit | — | 6.7.2.1 + ABI |

Nơi sống: **`ast.rs` TyTab** (`size/align/data_align`). Đổi model = **tham số hóa TyTab**,
KHÔNG rải điều kiện (luật kiến trúc).

### II-3. Calling convention — AAPCS64 (bảng register + phân loại)
| tham số | giá trị | nguồn |
|---|---|---|
| integer/pointer arg regs | x0–x7 (NGRN 0–7) | AAPCS64 §6.4 |
| FP/SIMD arg regs | v0–v7 (NSRN 0–7) | §6.4 |
| return | x0 (+x1 cho 16B), v0 | §6.4 |
| stack arg (NSAA) | tràn sau x7/v7, align 8 | §6.4 |
| sp trước `bl` | thẳng 16 byte | §6.2.2 |
| callee-saved | x19–x28, x29(fp), x30(lr) | §6.1.1 |
| composite tràn khóa NGRN=8 (C.11); HFA tràn KHÔNG khóa | — | §6.8 rule C.11 |
| prologue | `stp x29,x30,[sp,#-16]!` | §6.2.2 |

Nơi sống: **`codegen/arm64_elf.rs`**. Thuật toán offset arg sống **3 nơi phải khớp từng byte**
(codegen call / codegen spill / parser va_off) — sửa 1 = sửa 3 + `gate abi`.

### II-4. Object format — ELF / section (AArch64 Linux)
| hằng | giá trị | nguồn |
|---|---|---|
| sections | `.text`/`.rodata`/`.data`/`.bss` | System V ABI |
| symbol: **KHÔNG** underscore (khác Darwin) | — | ELF |
| relocation local | `adrp`+`:lo12:` (PAGE/PAGEOFF) | AArch64 ELF |
| extern/GOT | `:got:`+`:got_lo12:` | ELF |
| TLS | `:tprel_*` / TLS model | ELF TLS |

Nơi sống: **`codegen/arm64_elf.rs`**. (Đặc sản Darwin cũ — `_`, `@PAGE`, `@TLVPPAGE`,
variadic-lên-stack — đã BỎ khi drop Mach-O; ghi ở CLAUDE.md để tránh nhầm.)

### II-5. Arch — AArch64 instruction/encoding constants
Register file (x0–x30, sp, v0–v31), immediate ranges (add/sub 12-bit, logical bitmask,
branch offset ±128MB), condition codes (eq/ne/lt…), addressing modes (`[base,#off]`,
`[base,index,lsl]`). Nguồn: **ARM ARM (DDI 0487)**. Nơi sống: `codegen/arm64_elf.rs` (asm text).

### II-6. GCC/vendor spec — bề mặt lệch chuẩn (`EXT(...)`)
| món | trạng thái | marker |
|---|---|---|
| stmt-expr `({...})`, `__extension__` | DÙNG | `EXT(gcc)` |
| `__attribute__((aligned/packed/weak/alias/transparent_union))` | DÙNG | `EXT(gcc)` |
| `__attribute__((mode(QI/HI/SI/DI/word/SF/DF/TF)))` → remap width | DÙNG (bảng machmode Vế-II; TI/XF reject) | `EXT(gcc)` `parser.rs apply_mode` |
| `__builtin_*` (whitelist), `typeof`, `__GNUC__=4`, `types_compatible_p` | DÙNG chọn lọc | `EXT(gcc)` |
| labels-as-values (`&&label`, `goto *e`), stmt-expr, range `case lo…hi`/`[lo…hi]`, elvis `?:` | DÙNG | `EXT(gcc)` |
| extended-asm (template + constraint hẹp, musl-critical) | DÙNG subset | `EXT(gcc)` |
| `vector_size`, `scalar_storage_order`, nested-func, `mode(TI/XF)` | **REJECT sạch** → NOT-IMPL | `EXT(gcc)` |

Nơi sống: **`src/ext.rs`** + điểm chạm đánh `EXT(...)`. Kiểm chứng bằng phép cắt:
bỏ ext.rs + nhánh marker → phần còn lại pass nguyên suite C89 (`grep 'EXT(' src/` phủ 100%).

---

# PHẦN III — KEYSTONE: correctness-by-construction & vì sao Gödel nằm ngoài

**Mệnh đề (Vu 2026-08-21):** nếu KHÔNG một dòng `src/` nào nằm ngoài không gian
{theory-fact ∪ spec-fact} — mỗi dòng là hiện thực **trung thành** của một định lý (vế I)
hoặc một hằng-spec (vế II) — thì zcc **tất yếu pass mọi suite**. Không thể phủ định.

**Vì sao đúng (điều kiện chặt — "trung thành" là bản lề):** suite = differential vs referee
(`cc`); cả zcc lẫn referee đều là **bóng của CÙNG một spec** (ISO C99 + AAPCS64 + ELF + AArch64)
trên cùng nền toán. Hai bóng trung thành của một vật thì trùng nhau — mismatch ⟹ một bên đọc sai
spec ⟹ **bug NẰM TRONG không gian** (hiện thực sai, không phải "ngoài không gian"), bắt được bằng
gate. Ba điều kiện của "trung thành":
1. **Faithfulness** — code thực sự hiện thực ĐÚNG định lý; hằng thực sự khớp ĐÚNG dòng spec.
   Bug không nấp "ngoài không gian" mà nấp ở "hiện thực SAI bên trong không gian".
2. **Completeness** — theory+spec phủ trọn ngôn ngữ suite chạm; lỗ hổng = **NOT-IMPL** (reject
   trung thực), KHÔNG phải miscompile. Đúng kỷ luật 2-fact: **0 FAIL**, không đòi 0 NOT-IMPL.
3. **Shared ground truth** — zcc và referee cùng gốc spec ⟹ agreement là tất yếu, không may rủi.

⟹ Toàn bộ cỗ máy engineering (sci-gate cho vế-I, differential-vs-referee cho vế-II, evidence-trail)
CHÍNH LÀ phép **kiểm toán cơ học tính trung thành**. Triết học và bộ test là MỘT, nhìn từ hai phía.

**Gödel (bất toàn) tuy đúng nhưng nằm NGOÀI quan hệ compiler↔suite.** Bất toàn chặn: một hệ hình
thức đủ mạnh không tự chứng minh được tính nhất quán của CHÍNH nó / mọi mệnh đề số học đúng. Quan hệ
compiler↔suite KHÔNG phải bài toán đó:
- **Per-case decidable** — chạy zcc và referee trên input cụ thể rồi so: hữu hạn, dừng.
- **Correctness-by-construction** chứng ở tầng rewrite-rule / không-gian-cấu-trúc-hữu-hạn — mỗi mảnh
  đều decidable (lý do 5 pass được CHỌN để ở trong fragment decidable, §C1). Rice/Halting/Gödel chỉ
  cắn nếu ta đòi thuật toán quyết định tương đương MỌI chương trình, hoặc bắt hệ tự-chứng-mình — ta
  KHÔNG làm thế.
- **Lối thoát khỏi tự-quy-chiếu = oracle NGOÀI.** Differential dùng referee độc lập: zcc không bao
  giờ phải tự chứng mình nhất quán, chỉ phải ĐỒNG Ý với một nhân chứng độc lập trên input cụ thể.
  Gödel cấm một hệ tự chứng consistency; KHÔNG cấm hai hệ độc lập đồng ý trên một vị từ decidable.
  → Cùng lý do CLAUDE.md rút Claude khỏi trust-path (kẻ-kể-chuyện-không-tin-được → chỉ verdict-cơ-học
  mới hợp lệ): dời trọng tài ra NGOÀI hệ là cách né đồng thời cả Gödel lẫn nghịch lý self-trust.

**Hệ quả DEBUG (Vu 2026-08-21) — fix THEO PHÂN RÃ, cấm patch cảm tính.** Khi zcc fail
suite (nhất là csmith/yarpgen), GIẢ ĐỊNH lý thuyết cho feature đó hợp lý ⟹ fail chỉ có
thể do 1 trong 3 (hoặc kết hợp), THỨ TỰ ưu tiên điều tra: **(1)** từ theorem phân rã ra SAI
control-flow/algorithm → có ≥1 LOC **nằm ngoài theorem** (vế I); **(2)** **spec-constant**
ISO/OS/arch apply SAI (vế II); **(3)** test/oracle/referee/generator lỗi hoặc thu input rác
(xác suất THẤP, ≠0) — RÀNG bởi LUẬT SUY-ĐOÁN-TỘI: compiler có tội tới khi chứng minh vô tội,
nên cause-3 là hạng CHÓT, chỉ tuyên sau proof ĐA CHIỀU cơ học + trọng tài độc lập; cấm dùng
làm phản xạ đổ lỗi, cấm cớ "clang/gcc cũng fail". Ta code theo phân rã ⟹ fix theo phân rã:
ĐỊNH VỊ lỗi bằng phép đo cơ học (bisect pass/module, diff asm, seek case) TRƯỚC, phân loại
vế-I/II/III SAU, sửa đúng chỗ đó. Nếu sửa mà phải thêm dòng-không-map-định-lý → sai hướng.
MEASURE đè mọi hypothesis — hypothesis-fix đầu tiên sai là bình thường, cứ đo tiếp (case mẫu
pr43220: đoán CSE-vế-I → đo bác bỏ → thực ra vế-II hằng frame-layout ở backend). (Chi tiết:
memory `zcc-debug-by-decomposition`.)

---

*Vu 2026-08-21: "1(theory→control-flow/data-structure/algorithm) ⊕ 2(iso/os/abi/arch/gcc spec
→ constant/param/value-table) = zcc source code — nếu chỉ giữ 1 luật trong CLAUDE.md thì giữ luật
này." + "phủ 250+ app dễ; pass csmith/yarpgen mới khó — chục compiler cùng cỡ vẫn trượt."
Vu sẽ bổ sung một list dài — merge vào đúng Phần/Ngành/Bảng.*
