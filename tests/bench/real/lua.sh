#!/bin/sh
# lua.sh — the SECOND real program on the clock: the Lua 5.4 interpreter.
#
# WHY A SECOND ONE, AND WHY THIS ONE. `realprog.sh` put sqlite on the clock to
# escape the taxonomy suite's three structural blindnesses (L1i pressure,
# register pressure, working sets past L2). sqlite closed those, but it is still
# ONE shape: a query engine, branch-heavy, integer-and-pointer, and it touches
# floating point almost nowhere. Law 3c's coverage note names the gaps that
# remain by name, and three of them meet in an interpreter:
#
#   * DISPATCH — `luaV_execute` is a ~100-arm switch inside a loop, re-entered
#     every bytecode. The branch predictor, not the arithmetic, sets the pace,
#     and the arm bodies compete for registers across a switch the allocator
#     has to colour as one enormous function.
#   * TAGGED VALUES — every operand is a `TValue` union plus a type byte, so the
#     hot path is load / test / branch on a 16-byte object, never a plain int.
#   * FP IN THE HOT PATH — Lua numbers are C `double`. `nbody` below runs its
#     arithmetic THROUGH the VM, so the FP work is interleaved with dispatch
#     rather than sitting in a tight vectorisable kernel. The taxonomy suite has
#     no FP-heavy member; this is the arm that supplies one.
#   * GC AND POINTER CHASING — `btrees` allocates and collects for its whole
#     run, which is the memory behaviour the small kernels cannot produce.
#
# THE TWO BUILDS SEE THE SAME SOURCE. Two `-D`s are forced on BOTH sides, and
# neither is a favour to either compiler:
#   -DLUA_USE_JUMPTABLE=0 — Lua turns on computed `goto` (`&&label`) whenever
#     `__GNUC__` is defined, and zcc defines `__GNUC__=4`. Left alone the two
#     builds would not be the same program: gcc would run a threaded
#     interpreter and zcc a switch. Forcing 0 gives both the switch, which is
#     also the dispatch shape this benchmark exists to measure.
#   -D__extension__= — `loadlib.c` writes `(__extension__ (lua_CFunction)(p))`
#     under `#if defined(__GNUC__)`. zcc claims `__GNUC__` but does not
#     implement the `__extension__` keyword, so it fails with `undeclared
#     identifier: lua_CFunction`. That is a REAL zcc gap (2-line reproducer in
#     the header of this file's report); the define is the workaround that lets
#     the rest of the program be measured, applied to both sides so the
#     preprocessed source stays identical.
#
# CLEAN-INPUT LAW. Each script's stdout is compared between the two
# interpreters BEFORE any time is printed. A number taken from a binary that
# computes something else is not a measurement.
#
# INTERLEAVED, NOT BEST-OF-N-IN-A-ROW. This machine drifts about 5% run to run,
# so timing all of gcc and then all of zcc measures the drift as much as the
# compilers. Each round runs gcc once and zcc once, back to back, and the
# minimum is taken per side across rounds — the pair is always sampled in the
# same weather. The fork+exec floor is MEASURED (`timeit 20 /bin/true`) and
# subtracted from both sides before the ratio is taken.
#
# THE COMPILER MUST BE THE RELEASE BUILD (the §CP finding: a debug zcc is ~9x
# slower and holds a debug allocator's memory). `ZCC_REL=1` builds it.
#
# ONE COMMAND, from the repo root:
#   ZCC_REL=1 sh tests/box.sh s 'sh /work/zcc/tests/bench/real/lua.sh' 2>/dev/null
#
# The source tree is NOT in the repo. Fetch it once into the suite cache:
#   mkdir -p ~/.cache/zcc-suites/lua && cd ~/.cache/zcc-suites/lua \
#     && curl -sSLO https://www.lua.org/ftp/lua-5.4.7.tar.gz && tar xzf lua-5.4.7.tar.gz
# The three benchmark scripts live beside it in ~/.cache/zcc-suites/lua/bench/.
# THE REFEREE IS `gcc -O2`, and the level is a decision rather than a default
# (MEASURED M48). Real software is built at -O2: it is the level every
# distribution, every `./configure` and every `Makefile` reaches for, so it is
# the only level a claim about zcc's generated code can be read against without
# misleading someone. This module scored against -O1 until 2026-08-29, which was
# the fair comparison for a compiler with no loop or vector passes and answers a
# question about the COMPILER rather than about the code a user would get.
# `GCC_OPT=-O1 sh <this>` restores the old column — but a number taken at one
# level does not transfer to the other, so do not read them together.
set -u
LUA="${LUA_DIR:-/suites/lua}"
SRC="$LUA/lua-5.4.7/src"
BENCH="${LUA_BENCH:-$LUA/bench}"
W="${ZCC_WORK:-/work/zcc}"
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-gcc}"
GCCO="${GCC_OPT:--O2}"   # MEASURED M48 — the referee level; see the header
ROUNDS="${ROUNDS:-5}"   # interleaved pairs per script
BN="${BN:-1}"           # build repetitions — one is enough; the build is 33 files
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT

[ -f "$SRC/lvm.c" ] || { echo "no $SRC/lvm.c — see the fetch command in this file's header"; exit 2; }
ls "$BENCH"/*.lua >/dev/null 2>&1 || { echo "no $BENCH/*.lua"; exit 2; }

# Both instruments are built by the REFEREE, never by the compiler under test.
$GCC -O2 -w -o "$T/maxrss" "$W/tests/bench/maxrss.c" || { echo "maxrss build failed"; exit 2; }
$GCC -O2 -w -o "$T/timeit" "$W/tests/bench/timeit.c" || { echo "timeit build failed"; exit 2; }
M="$T/maxrss"; TI="$T/timeit"

FLOOR=$("$TI" 20 /bin/true | awk '{print $2}')

# `<us> <peak_kb> <rc>` on fd 3; the child's own output is left alone.
meas() { out=$1; shift; "$M" "$@" >"$out" 2>>"$T/err" 3>"$T/m"; cat "$T/m"; }
# one timed run of one binary, floor subtracted
once() { "$TI" 1 "$@" | awk -v f="$FLOOR" '{ d=$2-f; print (d>0?d:0) }'; }

# `luac.c` is the separate bytecode compiler and has its own main(); everything
# else in src/ is the interpreter's standard static build.
SRCS=$(ls "$SRC"/*.c | grep -v '/luac\.c$')
D="-DLUA_USE_LINUX -DLUA_USE_JUMPTABLE=0 -D__extension__="

echo "############################################################"
echo "# REAL PROGRAM #2 — Lua 5.4.7 interpreter, built and RUN    #"
echo "############################################################"
echo "timing floor (fork+exec of /bin/true, best of 20): ${FLOOR}us"
echo
echo "== BUILD (compile wall · compiler peak RSS · binary bytes) =="
printf "%-10s %10s %12s %12s\n" compiler wall_us peak_rss_kb bytes

build() { # name  cc  extra-cc-flags...
    nm=$1; shift
    best_ms=; best_kb=; i=0
    while [ "$i" -lt "$BN" ]; do
        r=$(meas /dev/null "$@") || { echo "$nm: COMPILE FAILED"; tail -20 "$T/err"; exit 1; }
        ms=$(echo "$r" | awk '{print $1}'); kb=$(echo "$r" | awk '{print $2}')
        { [ -z "$best_ms" ] || [ "$ms" -lt "$best_ms" ]; } && best_ms=$ms
        { [ -z "$best_kb" ] || [ "$kb" -gt "$best_kb" ]; } && best_kb=$kb
        i=$((i+1))
    done
    echo "$best_ms $best_kb"
}

r=$(build gcc $GCC $GCCO -w $D -I"$SRC" -o "$T/lua_gcc" $SRCS -lm -ldl -Wl,-E)
g_ms=${r% *}; g_kb=${r#* }; g_sz=$(wc -c < "$T/lua_gcc")
printf "%-10s %10s %12s %12s\n" "gcc $GCCO" "$g_ms" "$g_kb" "$g_sz"

r=$(build zcc $ZCC -w $D -I"$SRC" -o "$T/lua_zcc" $SRCS -lm -ldl -Wl,-E)
z_ms=${r% *}; z_kb=${r#* }; z_sz=$(wc -c < "$T/lua_zcc")
printf "%-10s %10s %12s %12s\n" "zcc" "$z_ms" "$z_kb" "$z_sz"
printf "%-10s %10s %12s %12s\n" "RATIO" \
  "$(awk "BEGIN{printf \"%.3f\", $z_ms/$g_ms}")" \
  "$(awk "BEGIN{printf \"%.3f\", $z_kb/$g_kb}")" \
  "$(awk "BEGIN{printf \"%.3f\", $z_sz/$g_sz}")"
echo

echo "== RUN ($ROUNDS interleaved pairs; output differentially checked first) =="
printf "%-10s %9s %9s %7s %11s %11s %7s\n" script gcc_us zcc_us t_ratio gcc_rss_kb zcc_rss_kb r_ratio

tot_g=0; tot_z=0; diverge=0
: > "$T/ratios"
for f in "$BENCH"/*.lua; do
    p=$(basename "$f" .lua)
    # CORRECTNESS FIRST — and the RSS reading comes off this same pair of runs.
    gr=$(meas "$T/go" "$T/lua_gcc" "$f")
    zr=$(meas "$T/zo" "$T/lua_zcc" "$f")
    if ! cmp -s "$T/go" "$T/zo"; then
        printf "%-10s %s\n" "$p" "DIVERGE"
        echo "  gcc: $(head -c 200 "$T/go")"
        echo "  zcc: $(head -c 200 "$T/zo")"
        diverge=$((diverge+1))
        continue
    fi
    rest=${gr#* }; gk=${rest%% *}
    rest=${zr#* }; zk=${rest%% *}
    # INTERLEAVED best-of-ROUNDS: one gcc run and one zcc run per round.
    gm=; zm=; i=0
    while [ "$i" -lt "$ROUNDS" ]; do
        a=$(once "$T/lua_gcc" "$f"); b=$(once "$T/lua_zcc" "$f")
        { [ -z "$gm" ] || [ "$a" -lt "$gm" ]; } && gm=$a
        { [ -z "$zm" ] || [ "$b" -lt "$zm" ]; } && zm=$b
        i=$((i+1))
    done
    tot_g=$((tot_g+gm)); tot_z=$((tot_z+zm))
    tr=$(awk "BEGIN{ if($gm>0) printf \"%.3f\", $zm/$gm; else print \"-\" }")
    rr=$(awk "BEGIN{ if($gk>0) printf \"%.3f\", $zk/$gk; else print \"-\" }")
    printf "%-10s %9s %9s %7s %11s %11s %7s\n" "$p" "$gm" "$zm" "$tr" "$gk" "$zk" "$rr"
    echo "$p $tr" >> "$T/ratios"
done

echo "---"
printf "%-10s %9s %9s %7s\n" TOTAL "$tot_g" "$tot_z" \
  "$(awk "BEGIN{ if($tot_g>0) printf \"%.3f\", $tot_z/$tot_g; else print \"-\" }")"
# Both are printed for the reason realprog.sh prints both: the TOTAL is
# sum-weighted and the GEOMEAN is not, and quoting either alone has misled this
# project once already.
awk '{n++; r=$2; s+=log(r); a[n]=r; if(r>worst){worst=r; wn=$1}}
     END{ if(n==0){print "GEOMEAN: no scripts"; exit}
       for(i=1;i<=n;i++)for(j=i+1;j<=n;j++)if(a[j]<a[i]){t=a[i];a[i]=a[j];a[j]=t}
       med=(n%2)?a[(n+1)/2]:(a[n/2]+a[n/2+1])/2;
       printf "GEOMEAN over %d scripts: %.4f | median %.3f | worst %s %.3f\n",
              n, exp(s/n), med, wn, worst }' "$T/ratios"
echo
echo "Three scripts are three ARMS, not three samples: nbody is FP-through-dispatch,"
echo "btrees is allocation and GC, strtab is hashing and string interning. Read the"
echo "per-arm column before the geomean — a compiler can be at parity on one and not"
echo "the others, and that is the thing the taxonomy suite could not tell anyone."
[ "$diverge" = 0 ] || { echo "LUA RED ($diverge scripts DIVERGE)"; exit 1; }
echo "LUA OK (0 DIVERGE)"
