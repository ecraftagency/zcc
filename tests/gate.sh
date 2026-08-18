#!/bin/sh
# gate.sh — T2 (SOP v3): dispatcher gate theo VÙNG SRC vừa sửa.
# Chỉ chạy gate sở hữu vùng, không chạy gate không liên quan.
# Dùng: tests/gate.sh <vùng> [vùng...]   |   tests/gate.sh all
# Vùng (khớp bảng gate-map trong SOP):
#   lex | cpp | decl | layout | uac | abi | decay | ext | cases | all
# Ghi chú interface thật: shape.sh chạy TRỌN 3 lớp lex/decl/layout một phát
# (không nhận arg) — gọi vùng nào trong 3 lớp đó đều nổ shape.sh nguyên con,
# vẫn đơn vị phút. Gate ELF trong box: chạy qua probe.sh với ZCC env.
set -u
D=$(cd "$(dirname "$0")" && pwd)
[ $# -ge 1 ] || { sed -n '2,9p' "$0"; exit 2; }
rc=0
run_gate() {
    echo "== GATE: $*"
    ( cd "$D/.." && sh "$@" ) || { rc=1; echo "== GATE ĐỎ: $*"; }
}
for v in "$@"; do
    case $v in
        lex|decl|layout|shape|tytab) run_gate tests/shape.sh ;;
        cpp|preprocessor)            run_gate tests/cpp.sh ;;
        uac|alg|const)               run_gate tests/alg.sh ;;
        abi|call|spill|va_arg|ret)   run_gate tests/abi.sh ;;
        decay|type)                  run_gate tests/decay.sh ;;
        ext)                         run_gate tests/run.sh ext ;;
        cases)                       run_gate tests/run.sh cases ;;
        all) for g in tests/shape.sh tests/cpp.sh tests/alg.sh tests/abi.sh tests/decay.sh; do run_gate $g; done
             run_gate tests/run.sh cases; run_gate tests/run.sh ext ;;
        *) echo "gate.sh: vùng lạ '$v' (xem bảng trong header)"; rc=2 ;;
    esac
done
[ $rc -eq 0 ] && echo "T2 XANH: mọi gate được gọi đều pass"
exit $rc
