# zcc — hiến chương dự án

C89+ compiler viết bằng Rust. Tác giả: Vu (xưng hô "mày/tao", trả lời tiếng Việt, thuật ngữ kỹ thuật giữ tiếng Anh).

## 2 yêu cầu tối thượng (mọi quyết định quy về đây)

1. **Strict compliance C99** (Vu nâng chuẩn gốc từ C89, 2026-08-18 — C89 tự động là tập con; 4 món C99 cuối mua khi M18 mở băng, xem MILESTONES "C99-ĐỦ") — ngữ nghĩa đúng spec; phần mở rộng (C11/vendor) chỉ tồn tại khi có phần mềm thật đòi, marker `EXT(gcc/clang/apple)` giữ nguyên; ghi chú `C99:` trong code là chú thích sư phạm, không phải ranh giới lệch chuẩn. Target đã đạt: AArch64 macOS Mach-O + AArch64 ELF Linux; x86_64 HOÃN.
2. **Ít LOC nhất có thể** — không optimization pass (ngữ nghĩa -O0), không tính năng viết trước khi có file `.c` thật đòi, không abstraction đón đầu, zero external crate. Trần cứng: **10k LOC** — CHỈ TÍNH `src/` (compiler = định lý, phải đọc được). `tests/` là procedure kiểm chứng: code C, suite, script harness đều để trong đó, KHÔNG trần (Vu 2026-08-18: "phình 100k cũng được — không ai ngó, chỉ cần chạy đúng").

Khi 2 yêu cầu xung đột: compliance thắng LOC.

## Luật kiến trúc (bất biến)

```
main.rs (driver) → lexer → parser → AST (arena + NodeId(u32), không Box chằng chịt) → codegen/<target> → .s text
```

- **Boundary frontend/backend = `src/ast.rs`** (AST + TyTab). Frontend DỰNG, backend chỉ ĐỌC; hai tầng không import lẫn nhau. Layout size/align nằm trong TyTab (lock LP64; cần khác thì tham số hóa TyTab, không rải điều kiện).
- **Mỗi target một file** dưới `src/codegen/`; `codegen/mod.rs` là cửa duy nhất (`emit(&Ast) -> String`). ABI/section/asm syntax nằm TRỌN trong file target. Thêm target = thêm file + nhánh match + nhánh toolchain bên driver.
- **Luật driver drop-in**: `CC=zcc` phải cắm vào build system thật (configure/make/cmake) của phần mềm mục tiêu mà KHÔNG sửa một dòng build file. Driver phối hợp toolchain host TRỰC TIẾP (as → ld, không qua cc driver). Bề mặt flag mua theo test-first: flag được implement khi build system thật dùng nó và nuốt sẽ làm sai; còn lại nuốt im lặng — nhưng TUYỆT ĐỐI không nuốt nhầm flag có tham số đi kèm (lệch một cái là ăn nhầm input file). Diagnostic format `file:line:` chuẩn + exit code đúng (configure grep stderr).
- Single crate, không workspace.

## Luật decouple extension (sư phạm: ranh giới ISO C / phương ngữ vendor phải nhìn được)

- Logic extension có thịt sống trong **`src/ext.rs`**; core chỉ gọi qua hàm `ext_*`.
- Điểm chạm không tách file được BẮT BUỘC đánh marker **`// EXT(gcc)`** / `EXT(clang)` / `EXT(apple)` / `EXT(c99)` — `grep 'EXT(' src/` phải phủ 100% bề mặt lệch chuẩn. Attribution theo phương ngữ GỐC.
- Kiểm chứng bằng phép cắt: bỏ ext.rs + các nhánh marker → phần còn lại vẫn pass nguyên suite C89. Không tự tuyên bố.
- Test extension ở `tests/ext/` (trọng tài `cc` không `-std=c89`), không trộn vào `tests/cases/`.

## Luật test & proof

- **MATHEMATIC FOUNDATION (luật gốc)**: mọi feature của compiler phải liên kết hoặc rút ra từ một nguyên lý — lý thuyết trình biên dịch, toán rời rạc, tập hợp, automata (lexer = ngôn ngữ chính quy, preprocessor = hệ viết lại hạng, parser = văn phạm phi ngữ cảnh, UAC = semilattice, ABI = automaton hữu hạn, codegen = simulation per-node). Test nội bộ phải phủ math proof tối đa có thể: feature mới trước hết hỏi "nó thuộc không gian nào, vét được không, gate nào giữ nó".
- **Test-first ép LOC**: compile chương trình thật TRƯỚC, vỡ ở construct nào mới implement construct đó.
- **Mọi kết luận đúng/sai đều differential**: trọng tài `cc` (spec bằng xương thịt) hoặc oracle độc lập; diff tại điểm UB là vô nghĩa — generator phải lọc UB trước.
- **Gate khoa học** (vét cạn không gian hữu hạn — chạy khi đụng vùng tương ứng): abi.sh (ABI automaton, link CHÉO — lỗi ABI cùng-compiler tự triệt tiêu), alg.sh (UAC semilattice), cpp.sh (hệ viết lại hạng), shape.sh (lexer/declarator/layout). "Vét cạn" = vét không gian CẤU TRÚC + mẫu biên không gian giá trị — nói "proof" phải kèm câu này.
- **Suite ngoài**: fail mới ngoài baseline đã triage = bug zcc cho tới khi chứng minh ngược lại; baseline không phải thùng rác giấu bug.
- **Tối ưu vòng test**: khi triage/fix, chạy lại ĐÚNG case/unit fail lần trước, KHÔNG chạy full suite; full suite chỉ chạy một lần cuối để đóng sổ (và chạy nền, không ngồi chờ). Suite nặng chạy TUẦN TỰ, không đè nhau tranh core.
- **Thuật toán offset arg sống ở 3 nơi phải khớp từng byte** (codegen call, codegen spill, parser va_off) — sửa 1 = sửa 3 + chạy abi.sh.
- Đối chiếu đáp án mẫu bất cứ lúc nào: `clang -S -O0 -std=c89 foo.c`.
- **Quy tắc số liệu của Vu**: mọi con số/quyết định phải suy ra được từ tiền đề đã tuyên bố, không magic number không nguồn gốc.

## Đặc sản ABI Darwin/AArch64 (sai là crash khó hiểu — đọc trước khi đụng codegen)

- Symbol có `_`; KHÔNG địa chỉ tuyệt đối — global qua `adrp @PAGE`/`@PAGEOFF`, extern qua GOT, TLS qua `@TLVPPAGE` + tlv_get_addr.
- **Variadic: tham số vô danh đi LÊN STACK** (đặc sản Apple, ngược Linux ARM64); named args x0–x7; scalar named trên stack PACK theo natural alignment; composite tràn khóa NGRN=8 (C.11) nhưng HFA tràn KHÔNG khóa.
- AAPCS64: args x0–x7, return x0, float v0–v7, sp thẳng 16 byte trước `bl`; prologue `stp x29, x30, [sp, #-16]!`.
- `char` mặc định signed trên Darwin. Sections: `__TEXT,__text` / `__TEXT,__cstring` / `__DATA,__data`.

## Index (chi tiết nằm ngoài, không ghi vào đây)

- **`MILESTONES.md`** — thang milestone 3 giai đoạn, thành tích + quyết định hệ trọng từng M, ngân sách LOC, sổ nợ.
- **`tests/README.md`** — sổ tài sản test: bản đồ 6 lớp proof, gate trong repo, suite công nghiệp + baseline, suite chính chủ nginx/redis, bẫy đã trả học phí.
- **`src/ext.rs` + `grep 'EXT(' src/`** — toàn bộ bề mặt lệch chuẩn hiện hành.
