# Sổ tài sản test của zcc

Nguyên tắc: **mọi tài sản test hoặc là script chạy được trong repo, hoặc được
ghi ở đây kèm cách dựng lại**. Không có tài sản mồ côi.

**zcc bản chất HỌC THUẬT** (Vu 2026-08-20): mỗi dòng code map tới một định lý
trình biên dịch — `[định lý toán/compiler] --compiled--> [rust source]
--compiled--> zcc`. Test chia HAI TẦNG:

1. **Tầng KIỂM ĐỊNH LÝ — sci-gate (ground truth, quan trọng HƠN corpus)**: vét
   cạn KHÔNG GIAN CẤU TRÚC để chứng nghiệm định lý. Tốc độ KHÔNG quan trọng,
   completeness là tối thượng (chạy cả ngày cũng chấp nhận). Phải MỞ RỘNG thêm.
2. **Tầng CHỨNG NGHIỆM THỰC TIỄN — corpus + app**: torture/csmith/linux/libc
   chỉ là MẪU thực tế, tầng dưới, xác nhận định lý không lệch đời thực.

**Tối giản (Vu 2026-08-20)**: framework cũ (probe/gate-overnight/SOP/ledger +
app-stack nginx/redis/git/sqlite + m8..m14) đã DẸP — over-automation làm
iteration chậm & đẻ file quá nhiều (luật đo tốc độ, CLAUDE.md). App giữ DUY
NHẤT musl libc (liên quan trực tiếp minimal-distro). **ELF = target
AUTHORITATIVE; Mach-O/mac hiện chỉ giữ để clang làm ORACLE** (có thể fork bỏ
Mach-O sau để chừa LOC).

```
tests/
├── shape.sh cpp.sh decay.sh alg.sh abi.sh   # SCI-GATE + gen_*.py (tầng định lý)
├── gate.sh                # dispatcher: gate.sh <vùng>  (chạy đúng gate sở hữu vùng)
├── box.sh                 # chạy 1 file / shell trong ELF box (iteration)
├── run.sh  cases/ ext/    # base differential + case viết tay (+ cases.known-fail)
├── suites/*.sh            # corpus ngoài (gate = FAIL ⊆ *.known-fail)
├── halfsuite.sh           # alias mỏng: = fullsuite.sh base (vòng nhanh)
└── fullsuite.sh           # RUNNER DUY NHẤT, 100% BOX: [TARGET] [SEEK] — tự build+docker
```

## Runner — 100% BOX (Vu 2026-08-20: box nhanh, runner mac đã bỏ)

`fullsuite.sh` là cửa DUY NHẤT: trên mac tự build zcc-ELF (musl,release) + `docker
run zcc-box` + gọi lại chính nó trong box. Mac chỉ còn để clang làm oracle ad-hoc,
KHÔNG còn runner mac (static-musl trong box gần free, mac 2.7s/case codesign/dyld).

**`sh tests/fullsuite.sh [TARGET] [SEEK]`** — SEEK đến từng TẦNG, không chạy lại toàn bộ:

| TARGET | chạy gì |
|---|---|
| `all` (mặc định) | sci + corpus + app |
| `sci` `corpus` `app` `base` | nhóm (`base` = run.sh cases+ext, vòng nhanh) |
| `shape` `cpp` `decay` `alg` `abi` | 1 sci-gate |
| `cases` `ext` | 1 base differential |
| `torture` `cts` | 1 corpus suite |
| `musl` | app libc |

**`SEEK`** (đối số 2, tùy chọn) = chuỗi con tên case → seek sâu vào TỪNG UNIT trong
1 suite. Vd: `fullsuite.sh torture pr22061`, `fullsuite.sh cases float`.
Áp cho cases/ext + mọi corpus suite (lọc `grep -F` trên danh sách file). Sci-gate
sinh case NỘI BỘ qua gen_*.py nên chưa nhận SEEK (mở rộng sau nếu cần).

**`sh tests/halfsuite.sh [SEEK]`** = alias `fullsuite.sh base [SEEK]` — vòng nhanh.

## Sci-gate — tầng kiểm định lý (chạy trong box qua fullsuite.sh sci)

| Gate | Nền tảng toán | Vét gì |
|---|---|---|
| `shape.sh` | ngôn ngữ chính quy + grammar automata + record-layout đệ quy | integer-literal 3.1.3.2 × escape × maximal-munch; declarator sâu ≤3 (có gọi qua fn-ptr); layout struct/union/bitfield tổ hợp member ≤3 + offset từng member |
| `cpp.sh` | hệ viết lại hạng (term rewriting, terminating nhờ sơn-xanh) | ma trận expansion (prescan/paste/stringize/rescan) + vét cạn số học `#if` (oracle kép zcc==cc==python) |
| `decay.sh` | type-derivation lattice (lvalue conversion 6.3.2.1) | 12 cách sinh expr array × 11 ngữ cảnh × 2 nhánh; oracle differential trên observable |
| `alg.sh` | semilattice UAC (3.2.1.5) + commuting-square (isomorphic oracle) | op × kiểu × kiểu × corner²: ~43k điểm runtime + ~21k fold; 4 phép so gồm fold↔runtime NỘI BỘ zcc (hai đường từ cùng AST phải gặp nhau) |
| `abi.sh` | automaton hữu hạn (AAPCS64 = máy trạng thái NGRN/NSRN/NSAA) | 292 case × 4 hướng LINK CHÉO zcc↔gcc — lỗi ABI cùng-compiler tự triệt tiêu, chỉ cross-link mới phơi |

**"Vét cạn"** = vét không gian CẤU TRÚC + mẫu biên không gian giá trị (không
phải toàn 2^64) — nói "proof" phải kèm câu này (giới hạn trung thực).

**Điểm yếu lý thuyết cần củng cố (Vu 2026-08-20)**: tầng ABI/register/memory-
layout/ELF KHÔNG có chuẩn ISO trấn giữ (chỉ AAPCS64 + ELF psABI). Ground truth
ở đây = spec ABI + **reference impl gcc/ld** (khai thác qua cross-link automaton
của abi.sh). abi.sh là guardian mỏng nhất → ưu tiên mở rộng: return HFA/composite,
C.11 split reg↔stack, variadic edge, over-align, tích đầy đủ 9×9 khi counter trộn.

## Corpus — tầng chứng nghiệm thực tiễn (fullsuite.sh corpus)

Khuôn chung: referee-filter (`cc` từ chối = ngoài scope → skip), differential
exit+stdout, **gate = FAIL ⊆ baseline `*.known-fail`** (triage từng dòng).
Chạy TUẦN TỰ (mỗi suite ăn đủ core).

| Suite | Nguồn (clone --depth 1) | Ghi chú baseline |
|---|---|---|
| `torture.sh` | gcc-mirror/gcc (`gcc.c-torture/execute`) | **2-fact 3-đường** (xem dưới): PASS \| NOT-IMPL (`torture.not-impl`, nêu tên) \| FAIL. Gate = **0 FAIL**. Đã bắt bug thật (pr60017, pr33631, va_arg HFA, pr92904 aligned) |
| `cts.sh` | c-testsuite/c-testsuite | oracle `.expected` (stdout-byte, không referee → rẻ/deterministic). 00162 `[const 5]`, 00219 `_Generic` (construct RIÊNG torture=0, Phase-3 pin), 00204 LD=fp128 (sổ nợ ELF) |

**Đã BỎ (2026-08-21, coverage-diff cơ học):** `kr` (UB của đáp án — diff-invalid), `nora` (1630 case, 0 construct riêng — fingerprint dominated bởi torture mọi cột), `chibicc` (41 case, dup construct; `_Generic` đã có ở cts), `tcc` (Darwin-lock `xcrun`→zombie chết trong box ELF). Bằng chứng: torture áp đảo mọi construct 1-2 bậc; construct RIÊNG duy nhất của cả 4 = `_Generic`, giữ qua cts. Bootstrap-compiler (ý tưởng tcc) tái sinh ở tầng **third-party build** (đối trọng slimcc), không phải tầng construct-corpus.

### torture 2-fact — hợp đồng phân loại (chống skip ngầm)

`torture.sh` KHÔNG dùng known-fail/skip nữa. Referee = `cc -std=c99 -w -O0`
(gcc trong box, trọng tài độc lập & native cho suite này). Mỗi case đúng 1 nhãn:

- **PASS** — zcc compile chương trình C99-hợp-lệ → binary exit 0 (self-check abort()).
- **NOT-IMPL** — không phải bug; ghi vào `torture.not-impl` NÊU ĐÍCH DANH lý do:
  - `oracle-invalid` — referee c99 tự từ chối/không-chạy-sạch (gcc-ext, target-
    specific, UB). Reason = dòng `error:` của gcc.
  - `zcc-reject` — referee OK nhưng zcc in `zcc:…` rồi exit 1, KHÔNG đẻ binary,
    KHÔNG crash (trung thực chưa cài). Reason = diagnostic zcc (`<case>:<ln>: msg`).
- **FAIL** — zcc NUỐT C99-hợp-lệ rồi sai/crash (chỉ số phải = 0): `runtime`
  (đẻ binary nhưng sai/abort), `backend` (exit 1 KHÔNG có `zcc:` → as/ld nghẹn
  asm rác), `crash` (panic/signal). Đây là cái reviewer sợ: nuốt-rồi-crash thay
  vì reject-lúc-compile.

**LUẬT BẢO TOÀN** (enforce, không phải lời hứa): `pass+not-impl+fail` phải =
số case nạp; mỗi case xuất hiện ĐÚNG 1 verdict. Case bốc hơi (worker chết/treo)
hoặc trùng → harness TỰ ĐỎ ngay, không cho xanh giả. → verdict xanh chỉ hợp lệ
khi đẳng thức đóng; reviewer/Vu đối chiếu 1 dòng, không cần tin Claude.

## App — musl libc (fullsuite.sh app)

`musl-box.sh` / `musl.sh`: build musl 1.2.5 + libc-test, differential
`F_zcc \ F_ref` (referee musl-gcc). LDBL64 port; sổ nợ `-shared`/.so, wide/mbc.
Là phần mềm thật DUY NHẤT giữ lại (nền minimal-distro) — test kỹ.

## float_h — lệch chuẩn CÓ SỔ (base differential)

zcc chọn `long double = double` trên ELF (HỢP LỆ C99 §5.2.4.2.2 "LD ≥ double";
MSVC cùng lựa chọn), `float.h` khai `LDBL_MANT_DIG=53` cho TỰ NHẤT QUÁN (memory
vẫn binary128 cho ABI interop). Linux `cc` dùng binary128 (113) → `cases/
float_h.c` diff, fail khách quan DUY NHẤT trong box, đã vào `cases.known-fail`.
Trên mac (Darwin LD=double) case này pass.

Nguyên tắc baseline: mỗi dòng known-fail phải có giải thích (bảng này / đầu file
`*.known-fail` / commit). **Baseline KHÔNG phải thùng rác giấu bug** — fail mới
không giải thích được = bug zcc cho tới khi chứng minh ngược lại (luật suy đoán
tội, CLAUDE.md).

## Bẫy đã trả học phí (đọc trước khi debug "ma")

- **Lỗi ABI cùng-compiler tự triệt tiêu** — integration test chạy ngon vẫn có
  thể sai ABI; chỉ link CHÉO zcc↔gcc (abi.sh) mới phơi.
- **Offset arg sống ở 3 nơi phải khớp từng byte**: codegen `call()`, codegen
  spill prologue, parser `va_off`. Sửa 1 = sửa 3 + chạy abi.sh.
- **Implicit-int cắt pointer/double**: libc thiếu prototype → return int → (1)
  pointer sxtw cắt 64-bit → segfault phụ thuộc ASLR (heisenbug); (2) return
  double ở d0 nhưng caller đọc x0 rác → sai LẶNG LẼ. Nghi: `nm -u` + đối chiếu
  `src/headers/*.h`.
- **Stale .o thế hệ cũ** → lỗi link "ma". `make distclean` trước khi debug link.
- **Diff tại điểm UB là vô nghĩa** — chương trình đọc stdin/argv: feed source
  làm stdin tự tạo UB (uninit var) = chính là lý do BỎ suite kr (đáp án UB).
- **"thiếu image zcc-box" mà `docker images` VẪN liệt kê** — index tên của docker
  bị stale: `docker image inspect zcc-box` fail nhưng inspect theo ID (716e3cce…)
  OK. Vá: `docker tag <ID> zcc-box:latest` (thao tác local, không destructive).
