#!/bin/sh
# gate.sh — dispatcher that selects a gate by the SRC AREA just modified.
# Runs only the gate owning that area, not unrelated gates.
# Usage: tests/gate.sh <area> [area...]   |   tests/gate.sh all
# Areas (matching the gate-map table):
#   lex | cpp | decl | layout | uac | abi | decay | ext | cases | all
# Note on the actual interface: shape.sh runs ALL 3 layers lex/decl/layout in one
# shot (it takes no arg) — invoking any of those 3 areas fires the whole of
# shape.sh, still on the order of minutes. ELF gate in the box: run via probe.sh
# with the ZCC env.
set -u
D=$(cd "$(dirname "$0")" && pwd)
[ $# -ge 1 ] || { sed -n '2,9p' "$0"; exit 2; }
rc=0
run_gate() {
    echo "== GATE: $*"
    ( cd "$D/.." && sh "$@" ) || { rc=1; echo "== GATE RED: $*"; }
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
        *) echo "gate.sh: unknown area '$v' (see the table in the header)"; rc=2 ;;
    esac
done
[ $rc -eq 0 ] && echo "GREEN: every invoked gate passed"
exit $rc
