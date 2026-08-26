#!/bin/sh
# latency.sh — MEASURE the latency of each instruction form on this core, in
# units of a plain `add`. The Side-II table R4.18 needs and cannot invent.
#
# WHY THIS EXISTS. `cost = |MIR|` is exact for SIZE and blind to TIME by
# construction (one MInst is one machine instruction), and twice now that
# blindness has cost a benchmark 40-64% — matmul at IDENTICAL instruction count,
# j3 with the same. The fix is a TIME model, and a time model needs latencies.
# Apple publishes no Software Optimization Guide for this core, so there is no
# table to transcribe: it has to be measured, and `MEASURED.md` is where the
# result lives. Inventing the numbers would be inventing provenance (Law 0).
#
# THE METHOD, and why it needs no clock frequency. Time a loop whose body is a
# chain of K copies of ONE instruction, each reading the register the previous
# one wrote. The chain cannot overlap, so wall time is K * latency * iterations,
# whatever the core does about width or reordering. Divide by the same
# measurement for `add x0,x0,#1` and the clock cancels: the answer is a RATIO,
# in units of a one-cycle ALU op. That is exactly the form the cost model wants —
# it compares shapes, it does not predict absolute nanoseconds.
#
# THE CONTROL. `nop` is measured too. A chain of `nop` has no dependence at all,
# so it reports the loop overhead rather than a latency; if that number is not
# far below the others, the harness is measuring itself and every row is suspect.
#
# Run in the box:  sh tests/bench/latency.sh
set -u
K="${K:-32}"       # instructions per chain — long enough to bury loop overhead
ITERS="${ITERS:-4000000}"
REPS="${REPS:-7}"
CC="${CC:-gcc}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT

# Each row: NAME and the instruction text. Every one is a SELF-DEPENDENCE on x0
# (or w0), so the chain is serial by construction. `setup` seeds the registers.
ops='
plain_add_imm|add x0, x0, #1
plain_add_reg|add x0, x0, x1
sub_reg|sub x0, x0, x1
and_reg|and x0, x0, x1
orr_reg|orr x0, x0, x1
eor_reg|eor x0, x0, x1
add_shifted|add x0, x0, x1, lsl #3
add_extended|add x0, x0, w1, sxtw
lsl_imm|lsl x0, x0, #1
lsl_reg|lsl x0, x0, x1
mul|mul x0, x0, x1
madd_multiplicand|madd x0, x0, x1, x2
madd_accumulator|madd x0, x1, x2, x0
csel|csel x0, x0, x1, eq
sxtw|sxtw x0, w0
uxtb|uxtb w0, w0
ubfx|ubfx x0, x0, #3, #8
mvn|mvn x0, x0
rev|rev x0, x0
sdiv|sdiv x0, x0, x1
udiv|udiv x0, x0, x1
load_l1|ldr x0, [x0]
load_regoff|ldr x0, [x0, xzr]
nop_control|nop
'

emit() {   # $1 = instruction text  -> writes $T/b.s
    {
        echo '	.text'
        echo '	.globl main'
        echo '	.type main, %function'
        echo 'main:'
        echo '	stp x29, x30, [sp, -16]!'
        echo '	mov x29, sp'
        # x0 starts at the self-referencing cell so `ldr x0,[x0]` chases itself;
        # every other form only needs it non-zero.
        echo '	adrp x0, cell'
        echo '	add x0, x0, :lo12:cell'
        echo '	mov x1, #1'
        echo '	mov x2, #1'
        echo '	cmp x1, x1'          # seed the flags once, for `csel`
        printf '\tmovz x9, #%d\n' $((ITERS & 65535))
        printf '\tmovk x9, #%d, lsl #16\n' $((ITERS >> 16))
        echo '.Lloop:'
        i=0
        while [ "$i" -lt "$K" ]; do printf '\t%s\n' "$1"; i=$((i + 1)); done
        # `subs` re-seeds the flags every iteration, which `csel` above reads.
        echo '	subs x9, x9, #1'
        echo '	b.ne .Lloop'
        echo '	mov w0, wzr'
        echo '	ldp x29, x30, [sp], 16'
        echo '	ret'
        echo '	.size main, .-main'
        echo '	.data'
        echo '	.p2align 3'
        echo 'cell:'
        echo '	.quad cell'          # points at itself: the load-to-use chain
    } > "$T/b.s"
}

time_ms() {  # best-of-REPS, in ms
    best=999999
    r=0
    while [ "$r" -lt "$REPS" ]; do
        t0=$(date +%s%N); "$1" >/dev/null 2>&1; t1=$(date +%s%N)
        d=$(( (t1 - t0) / 1000000 ))
        [ "$d" -lt "$best" ] && best=$d
        r=$((r + 1))
    done
    echo "$best"
}

echo "== instruction latency on this core, K=$K chain, ITERS=$ITERS, best of $REPS =="
echo "   ratios are in units of a dependent \`add x0,x0,#1\`; the clock cancels"
echo

base=
printf "%-20s %8s %8s\n" form ms "latency"
echo "$ops" | while IFS='|' read -r name text; do
    [ -n "$name" ] || continue
    emit "$text"
    if ! $CC -c "$T/b.s" -o "$T/b.o" 2>"$T/err" || ! $CC -o "$T/b" "$T/b.o" 2>>"$T/err"; then
        printf "%-20s %8s   (assembler rejected: %s)\n" "$name" - "$(head -1 "$T/err")"
        continue
    fi
    ms=$(time_ms "$T/b")
    # The first row IS the unit. Keep it in a file: this loop runs in a subshell.
    if [ ! -f "$T/base" ]; then echo "$ms" > "$T/base"; fi
    base=$(cat "$T/base")
    ratio=$(awk "BEGIN{ if($base>0) printf \"%.2f\", $ms/$base; else print \"-\" }")
    printf "%-20s %8s %8s\n" "$name" "$ms" "$ratio"
done

echo
echo "Read: a form at 1.00 is one dependent ALU op. \`nop_control\` measures the"
echo "harness, not the machine — if it is not far below 1.00 the rows are noise."
echo "Record what this prints in MEASURED.md with the date and the machine; it is"
echo "a fact about the core that measured it, not about AArch64."
