# zcc — hiến chương dự án

C89+ compiler viết bằng Rust. Tác giả: Vu (xưng hô "mày/tao", trả lời tiếng Việt, thuật ngữ kỹ thuật giữ tiếng Anh).

## Luật gốc số 0 — định lý phân rã source (nếu chỉ giữ 1 luật, giữ luật này. Chi tiết: `THEORY.md`)

```
zcc source  =  ( math / theory     → control-flow + data-structure + algorithm )
            ⊕  ( iso / os / arch / gcc spec → constant + param + value-table )
```

Mỗi dòng `src/` thuộc ĐÚNG một trong hai vế, không có dòng thứ ba: hoặc *thuật toán/cấu trúc/luồng* rút từ một định lý (map được về `THEORY.md` Phần I), hoặc *hằng/bảng-tra* chép từ một dòng spec (`THEORY.md` Phần II — không magic number không nguồn gốc). **Correctness-by-construction:** nếu không LOC nào nằm ngoài không gian {theory-fact ∪ spec-fact} và mỗi dòng là hiện thực TRUNG THÀNH của vế của nó, zcc **tất yếu pass mọi suite** — vì zcc và referee đều là bóng của cùng một spec; mismatch ⟹ bug trung-thành NẰM TRONG không gian, gate bắt được. Kỷ luật 2-fact giữ điều kiện này: **0 FAIL** (miscompile), NOT-IMPL cho lỗ hổng completeness. Phủ 250+ app = bằng chứng RẺ (khả dụng); pass csmith/yarpgen + sci-gate = bằng chứng correctness ĐẮT (chục compiler cùng cỡ vẫn trượt). Gödel/Rice/Halting nằm NGOÀI quan hệ compiler↔suite: differential dùng oracle ĐỘC LẬP nên zcc không bao giờ tự-chứng-mình — né cả bất toàn lẫn self-trust (cùng lý do Claude bị rút khỏi trust-path). **Hệ quả DEBUG:** fail suite ⟹ fix THEO PHÂN RÃ, cấm patch cảm tính — giả định lý thuyết feature hợp lý thì fail chỉ do (I) phân rã ra sai control-flow/algorithm → LOC ngoài theorem, và/hoặc (II) spec-constant apply sai, và/hoặc (III) test/oracle/referee/generator lỗi (hạng CHÓT, ràng bởi luật suy-đoán-tội: chỉ tuyên sau proof đa chiều, cấm phản xạ đổ lỗi test); định vị LOC bằng phép đo cơ học TRƯỚC, phân loại vế-I/II/III SAU — MEASURE đè hypothesis, fix-đầu-sai là bình thường cứ đo tiếp (chi tiết: memory `zcc-debug-by-decomposition`). **Hỏi "zcc dựa nền lý thuyết nào" → in `THEORY.md`; thêm định lý/hằng mới → cập nhật `THEORY.md`.**

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

- **LUẬT ĐO TỐC ĐỘ (Vu 2026-08-18, đứng TRÊN mọi luật test khác)**: một cơ
  chế iteration dù hàn lâm hay khoa học đến đâu, nếu MEASURE cho ra con số
  ngược lại — tức làm iteration CHẬM HƠN thao tác trực giác (detect bug →
  fix → test ngay lại đúng cái fail) — thì DẸP NGAY, dù là 10k/20k/100k dòng
  code quý báu. Học phí sống: SOP v3 4-tầng/B0-B1/harvest/regress bị diệt
  cùng ngày sinh vì chính nó làm bài test redis xếp hàng sau giấy tờ.
  Ritchie viết C compiler trên PDP-11 không cần cái nào trong số đó.
- **MATHEMATIC FOUNDATION (luật gốc)**: mọi feature của compiler phải liên kết hoặc rút ra từ một nguyên lý — lý thuyết trình biên dịch, toán rời rạc, tập hợp, automata (lexer = ngôn ngữ chính quy, preprocessor = hệ viết lại hạng, parser = văn phạm phi ngữ cảnh, UAC = semilattice, ABI = automaton hữu hạn, codegen = simulation per-node). Test nội bộ phải phủ math proof tối đa có thể: feature mới trước hết hỏi "nó thuộc không gian nào, vét được không, gate nào giữ nó".
- **Test-first ép LOC**: compile chương trình thật TRƯỚC, vỡ ở construct nào mới implement construct đó.
- **Mọi kết luận đúng/sai đều differential**: trọng tài `cc` (spec bằng xương thịt) hoặc oracle độc lập; diff tại điểm UB là vô nghĩa — generator phải lọc UB trước.
- **LUẬT SUY ĐOÁN TỘI (Vu 2026-08-20, học phí sống)**: compiler CÓ TỘI cho tới khi chứng minh vô tội. Mọi cáo buộc "lỗi oracle/generator/test, không phải zcc" phải có **proof đa chiều TRƯỚC khi tuyên** — nhiều công thức/góc nhìn độc lập chụm về CÙNG một kết quả (đúng chuẩn proof mà decay đã trả). Bản năng đổ lỗi cho test chính là thứ **che giấu bug compiler**. Chứng cứ: 2026-08-20 tao (Claude) tuyên 4 case nora fall-off là "oracle-invalid, diff tại UB" — KHÔNG proof, cẩu thả; double-check ra clang c89 CŨNG trả 0 → thực chất **bug codegen zcc-ELF** (không emit `return 0` khi rơi-khỏi-main). Hai lần sai: (1) sai khi lười không proof, (2) kể cả lúc tưởng đã "proof" thì proof đó cũng sai. **META-KẾT LUẬN (Vu): trong CÙNG một phiên Claude đưa hai nhận định mâu thuẫn → correctness-bằng-lời-khẳng-định-của-Claude là IMPOSSIBLE; chỉ correctness-bằng-verdict-cơ-học-differential mới khả thi. Claude là kẻ kể chuyện KHÔNG đáng tin → phải rút khỏi trust path: chỉ được DỰNG và CHẠY oracle, CÂM cho tới khi oracle lên tiếng. Luật ĐO-TRƯỚC-KHI-NÓI: cấm tuyên bất kỳ phân loại (bug/oracle/ext) nào trước khi một script đã in ra verdict. Nghịch lý nora là bằng chứng cơ chế ĐÚNG: phép đo đè bẹp lý luận sai — lỗi nằm ở chỗ mở mồm trước khi đo.** → Hệ quả: (a) "diff tại UB" chỉ được viện DẪN sau khi đã chứng minh điểm đó THỰC SỰ là UB/unspecified bằng spec + trọng tài, không hand-wave; (b) "clang/gcc cũng fail nên ta được fail" TUYỆT ĐỐI cấm dùng làm cớ — phải đào tận gốc vì sao trọng tài từ chối, có thể chính nó lộ edge-case; (c) exclude một case chỉ khi chứng minh nó nằm ngoài phạm vi cài đặt (IR + Optimization / phương ngữ vendor), bỏ nhầm case đại diện edge-case semantic là thảm họa.
- **Gate khoa học = TẦNG KIỂM ĐỊNH LÝ (ground truth, quan trọng HƠN corpus — Vu 2026-08-20)**: zcc bản chất học thuật, mỗi dòng code map tới một định lý trình biên dịch; sci-gate vét cạn KHÔNG GIAN CẤU TRÚC để chứng nghiệm định lý đó (corpus/csmith/linux chỉ là chứng nghiệm THỰC TIỄN, tầng dưới). Vét cạn khi đụng vùng tương ứng: `abi.sh` (ABI automaton, link CHÉO — lỗi ABI cùng-compiler tự triệt tiêu), `alg.sh` (UAC semilattice + commuting-square fold↔runtime = isomorphic oracle), `cpp.sh` (hệ viết lại hạng), `shape.sh` (lexer/declarator/layout — grammar automata), `decay.sh` (type-derivation lattice). "Vét cạn" = vét không gian CẤU TRÚC + mẫu biên không gian giá trị — nói "proof" phải kèm câu này. Dispatcher `gate.sh <vùng>`; chạy trong ELF box qua `box.sh`. Runner DUY NHẤT `fullsuite.sh [TARGET] [SEEK]` chạy 100% TRONG BOX (Vu 2026-08-20: box static-musl gần free; runner mac đã bỏ, mac chỉ để clang làm oracle) — TARGET seek đến từng tầng (sci|corpus|app|all | 1 gate | 1 suite | base), SEEK seek đến từng case; `halfsuite.sh` = alias `fullsuite.sh base`. **Sci-gate phải MỞ RỘNG thêm (Vu 2026-08-20)** — semantic-preservation, abstract-interpretation, formal type lattice… KHÔNG được co lại. App-stack (nginx/redis/git/sqlite) đã BỎ khỏi runner (chạy tay khi cần).
- **Suite ngoài**: fail mới ngoài baseline đã triage = bug zcc cho tới khi chứng minh ngược lại; baseline không phải thùng rác giấu bug.
- **LUẬT INPUT SẠCH (Vu 2026-08-20): mọi nguồn cội tội lỗi = input sai/rác thu thập trong lúc chạy suite.** Verdict PASS/FAIL vô giá trị nếu bản thân phép đo dựa trên dữ liệu rác (referee-filter skip nhầm, `2>/dev/null` nuốt lỗi, đếm nhầm nhãn, suite "xanh" mà không chạy gì). → Một verdict xanh CHỈ hợp lệ khi kèm **evidence trail cơ học** chứng minh đã làm việc thật: số artifact đẻ ra + checksum + exit-code quan sát, KHÔNG chỉ con số pass/fail. Chuẩn publish: "pass torture" phải kèm bằng chứng N binary ELF thật + tổng bytes codegen + mẫu chạy lại deterministic (vd 2026-08-20: torture box 16s ⇒ 1377 ELF thật/21MB/1694 case phủ trọn — nghi "16s là no-op" bị đè bằng manifest). Nghi ngờ tốc độ bất thường (nhanh/chậm) → ĐO exec-overhead, đừng đoán (mac clang compile+run 2.7s/lần vì codesign/dyld; Linux static-musl gần free → cùng suite mac 19ph vs box 16s).
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
- **`tests/README.md`** — sổ tài sản test (tối giản 2026-08-20): 2 tầng — sci-gate (kiểm định lý, ground truth) + corpus/base differential (chứng nghiệm thực tiễn); runner DUY NHẤT fullsuite.sh (100% box, [TARGET] [SEEK]); baseline + bẫy đã trả học phí. App-stack (nginx/redis/git/sqlite) đã bỏ khỏi runner (chạy tay).
- **`src/ext.rs` + `grep 'EXT(' src/`** — toàn bộ bề mặt lệch chuẩn hiện hành.
