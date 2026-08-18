# SOP test (v4 tối giản — Vu 18/8: "khoa học là để nhanh hơn, chậm là diệt")

Vòng lặp DUY NHẤT khi có bug (≤5 phút/iteration, lý tưởng ~30s):

1. Cắt repro ≤30 dòng `.c`, verdict differential (cc/gcc referee).
2. Fix src → `cargo build --release` (~10s; ELF thêm `--target aarch64-unknown-linux-musl`, linker rust-lld).
3. Test lại NGAY: host `cc` referee, hoặc box `docker exec` (image `zcc-box` đã bake sẵn deps — cấm apt-get trong probe).
4. Lặp tới khớp → repro vào `tests/cases/` + 1 dòng `ledger.md` → commit.

Suite ngoài (redis/nginx/git/musl) KHÔNG nằm trong vòng lặp: chạy đúng unit
đau, 1 lần chốt sổ, chạy NỀN (nohup + `docker wait` chain), tuần tự không đè
nhau; full suite chỉ ở release. Blast radius một patch: compile `tests/cases/`
bằng binary cũ vs mới cả 2 target rồi diff `.s` — 1 vòng for, không cần corpus.

Gate khoa học (host, nhanh): `tests/gate.sh <lex|cpp|uac|abi|decay|ext|cases|all>`
— chạy đúng gate vùng src vừa sửa; `abi` BẮT BUỘC khi đụng call/spill/va_arg
(luật 3-nơi-khớp-byte). Đo 18/8: abi+alg 53s, decay 1.4s, cases ~20s.

Luật sống sót (mỗi luật một học phí đã trả):
- Không án trong `ledger.md` → không patch. Fail chưa phân loại chữ ký = số mù.
- Referee phải chứng minh ĐÃ CHẠY (ran > 0); oracle rút gọn phải validate CẢ HAI phía.
- Án không đẻ script/file trong repo — probe 1-lần sống ở scratchpad/cache.
- Log sống ngoài container; artifact tái dùng, không rebuild thứ đã có.
- VirtioFS + make -j hay flake ("No rule to make target" dù file tồn tại) → retry make tuần tự.
- Một án một lúc: side-issue = 1 dòng ghi chú rồi quay về án chính.

Đã DIỆT 18/8 (dù hàn lâm): SOP v3 thang 4 tầng / ký hiệu B0-B1 / thang định vị
6 bậc / harvest + regress + corpus.manifest — ý tưởng còn trong git history,
ai cần thì đào. Ritchie viết C compiler trên PDP-11 không cần cái nào trong số đó.
