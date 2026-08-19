# Ledger — bảng án (SOP v3, bước 1)

Một án một dòng. Không án → không patch. Repro phải thu về ≤ 30 dòng C hoặc
1 lệnh trước khi viết fix (thang định vị trong SOP.md).

Schema: `id | ngày | chữ ký fail | repro | verdict(zcc/env/flaky/scope) | trạng thái | bằng chứng`

## Án đã đóng

| id | ngày | chữ ký | repro | verdict | trạng thái | bằng chứng |
|---|---|---|---|---|---|---|
| A1 | 2026-08-18 | segv flaky zcc khi compile testfixture 88 TU (sqlite) trong box | `make testfixture` trong zcc-box, lặp | env (stack 8MB, đệ quy parser đụng mép; sống chết theo cỡ env) — fix: driver stack 256MB tường minh (commit 9b197ff) | ĐÓNG 18/8 | `~/.cache/zcc-suites/p4-sqlite-20260818/SUMMARY.txt` — 30/30 iter sạch, không segv |

## Án mở

| id | ngày | chữ ký | repro | verdict | trạng thái | bằng chứng |
|---|---|---|---|---|---|---|
| G1 | 2026-08-18 | **Ghost diff-files/status** (t7112#57 `.gitmodules M 0{40}`, t0020#27 autocrlf, cụm rebase "unstaged changes", t1092 status --porcelain) | full-file trong box (subset `--run=57` VÔ HIỆU — học phí đã vào SOP) | **KHÔNG kết án được**: #57 flaky XUYÊN-SESSION cả hai phía (probe23 zcc 3/3 gcc 0/3, nhưng P3 CẢ HAI pass, probe28 CẢ HAI fail); nhiễu nền git t/ đối xứng zcc-86/gcc-106 dòng | **TREO theo lệnh điểm dừng Vu 18/8 khuya** — git t/ giữ vai suite với baseline known-fail, không đuổi flaky nữa | `probe26/27/28-20260818/*`, `p3-gitref-20260818/VERDICT.txt`; stash-create "ma" rate 1/400, anatomy postmortem sạch |
| G2 | 2026-08-18 | t6428 flag BROKEN phía zcc — soi lại: 2/2 test CÓ chạy và fail (flag harness P3 sai khi grep '^ok' = 0 do file chỉ có 2 test đều fail) | — | lỗi harness P3 (proof-of-run đếm thô), không phải án compiler | ĐÓNG 18/8 | `p3-gitref-20260818/z/t6428*.log` ("failed 2 among 2 test(s)") |

| R1 | 2026-08-18 | redis full-suite chết 86/156: `[exception] Failed to convert long double to string ('0' != '0.00000000000000001')` — moduleapi/misc, module zcc truyền "long double" 8-byte (deviation long double=double) vào glibc đợi fp128 16-byte | `cd /build/redis && ./runtest --single unit/moduleapi/misc` (box) | **schrödinbug có sổ collapse đúng dự đoán** — deviation long double ELF nay CÓ CHỦ NỢ trong rổ đóng băng | **ĐÓNG 18/8 đêm — ĐÃ MUA** (120 LOC, src 9334/10k): Ty::LDouble memory=binary128 16/16, reg=f64 canonical, nới/hạ __extenddftf2/__trunctfdf2 tại load/store/biên ABI, Slot::Q q-reg + va_arg quad + spill 16/16, driver -lgcc; bug thật thứ 2 tìm ra: default arg promotion nuốt long double→double (C99 6.5.2.2p6 phải GIỮ). Chứng: repro differential gcc KHỚP TUYỆT ĐỐI (`tests/cases/c99_long_double.c`); redis 22-unit fail cũ → 2025 ok, họ ldbl 0 err (moduleapi/misc án gốc sạch); Darwin 68 TU + ELF 48 TU ngoài vùng: 0 byte đổi; gate abi/alg/shape/cases xanh. Đuôi: musl GIỮ patch float.h 53 (precision khai thật, ABI 16-byte nay đã đúng) nhưng sysroot cần REBUILD (layout ldbl 8→16) khi đụng musl world | `p2-redis-20260818/redis-full.log`; vòng skip-unit `p2b-redis-20260818/` (5652 ok/123 err, họ long-double chiếm đa số: hash-field-expire 28, increx 15, moduleapi/reply 13, hash 8, incr 6); diff float.h: `tar -xzOf musl-1.2.5.tar.gz musl-1.2.5/arch/aarch64/bits/float.h` vs cây |
| R2 | 2026-08-18 | redis crash-report/stacktrace: `integration/logging` ×7 (`moduleapi/crash` = test KHÔNG tồn tại trong bản này → ledger book nhầm ×2) — server zcc không sinh stack trace khi crash | `cd /build/redis && ./runtest --single integration/logging` (box) | zcc — target-ABI ELF chưa đủ | **ĐÓNG 20/8 — ĐÃ FIX (đưa về test-set fullsuite)**: hoàn thiện ELF codegen/link cho stack unwinding + symbol resolution. 4 mảnh: (1) CFI `.cfi_*`→`.eh_frame` frame-pointer-based CFA=x29+16 (`arm64_elf.rs`); (2) `--eh-frame-hdr` link → PT_GNU_EH_FRAME cho runtime unwinder (`main.rs`); (3) `-rdynamic`→`--export-dynamic` (trước nuốt im) → dynsym có tên hàm; (4) `.type %function`+`.size .-name` → st_size≠0 để dladdr match địa chỉ GIỮA hàm (return-addr), không thì chỉ match byte đầu. Verify: standalone backtrace zcc FRAMES=9==gcc (trước=1); `runtest --single integration/logging` rc=0 cả 7 `[ok]` "All tests passed" (test đếm `bioProcessBackgroundJobs`×3 trong dump mọi-thread). Không miscompile (`.text` byte-identical, chỉ thêm metadata+2 link-flag). Blast-radius: +~35 LOC src (9354/10k); gate shape/cpp/alg/**abi** xanh với zcc mới → không regression feature đã chứng minh; redis 200 TU rebuild+chạy pass. | `~/.cache/zcc-suites/r2-frames-*` (FRAMES 1 vs 9), `r2-verify3` (7/7 ok) |
| R3 | 2026-08-18 | redis `unit/other` "Process title set as expected" — setproctitle không ăn trên binary zcc | `cd /build/redis && ./runtest --single unit/other` (box) | zcc | **ĐÓNG 20/8** | Root-cause: zcc thiếu predefine bare `__linux` (chỉ có `__linux__`); redis `setproctitle.c:51` + `config.h:213/341` dò `#if defined __linux` → khối impl Linux bị cắt → `spt_init`/`setproctitle` vắng mặt → `redisSetProcTitle` thành no-op. Fix: thêm `__linux`+`__unix` (bare) vào predefine Arm64Elf (`src/preprocess.rs`). Verify: redis build zcc mới có T `spt_init`/`setproctitle`, `runtest --single unit/other` rc=0 `[ok]: Process title set as expected`, All tests passed. Bằng chứng differential: `~/.cache/zcc-suites/r3-spt-*` (zcc thiếu SPT/link fail vs gcc ok), `r3-confirm-*` (H1 `-D__linux` → title=TEST), `r3-verify-*` |

## Nghi phạm P3 — ĐÓNG SỔ theo lệnh điểm dừng (Vu 18/8 khuya)

Phát hiện chốt sổ: gcc-only cũng có **106 dòng fail** (vs zcc-only 86) —
nhiễu nền git t/ trên VirtioFS/box là ĐỐI XỨNG hai phía; danh sách dưới đây
vì thế KHÔNG được coi là bug zcc khi chưa có chữ ký ổn định xuyên-run
xuyên-session (t7112#57 đã chứng minh flaky cả hai phía). Rổ suite chính
thức từ nay: nginx/redis/musl/git với baseline known-fail; không đuổi flaky.
Danh sách giữ làm tư liệu:

Nguồn: `p3-gitref-20260818/VERDICT.txt`, mục "fail CHỈ CÓ Ở ZCC". Tổng
fail-dòng zcc=515 vs gcc=535 — nền flaky/env dày, nên MỖI dòng dưới đây phải
rerun 3× phía zcc (và 1× phía gcc cùng test) trước khi mở án. Ưu tiên theo
cụm chữ ký (nhiều test cùng vùng = một bug gốc khả năng cao):

| Cụm nghi | Test (#fail) | Ghi chú |
|---|---|---|
| submodule | t1013(#18) t2013(#30 #34) t3512(#3 #4) t4255(#3) t6438(#3) t7112(→án G1) t7402(#2 #3 #6) | cụm dày nhất, khả năng 1 bug gốc |
| rebase | t3402(#11 #13) t3403(#5) t3404(#20 #23) t3418(#26–30) t3420(#22 #23 #37) t3429(#5) t3436(#3 #4 #5 #16 #17) t3437(#5) t5407(#9 #15–18) | cụm dày nhì |
| merge/reset | t6432(#2 #5 #6 #10) t7110(#4 #9 #10 #12) t7600(#18 #55 #59 #61 #62 #65) t7601(#62) t6428(→án G2) | |
| sparse-checkout | t1092(#5 #34 #61 #90 #103) t2080(#1) | |
| diff/apply/am | t4013(#179–183) t4051(#11) t4128(#5) t4151(#5 #6) | t4013 5 số liền kề = 1 chữ ký |
| refs/worktree | t1460(#6 #21 #36) t2400(#120) | |
| lẻ | t0001(#68) t0020(#27) t1002(#19) t3903(#57) t3905(#1) t5329(#14) t5520(#25) t7519(#28) t7900(#69 #70) | |
