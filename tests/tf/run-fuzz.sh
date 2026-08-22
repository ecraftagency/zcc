#!/bin/sh
# run-fuzz.sh — deploy the VERSION-LOCKED working tree to the provisioned Graviton box and
# run the milestone csmith + yarpgen gate at SEED cases each. The rsync of the current tree
# IS the version lock: whatever is checked out here is exactly what gets fuzzed.
#
#   ./run-fuzz.sh <public-ip>            # SEED defaults to 1000
#   ./run-fuzz.sh <ip> 10000             # milestone sweep — seed count is the 2nd argument
#   ./run-fuzz.sh --destroy <ip> 10000   # run, collect verdict, then terraform destroy EVERYTHING
#
# Prereq: `terraform apply` done, and cloud-init finished (see `terraform output ready_check`).
set -eu

DESTROY=0
[ "${1:-}" = "--destroy" ] && { DESTROY=1; shift; }
IP="${1:?usage: ./run-fuzz.sh [--destroy] <public-ip> [seed_count=1000]}"
SEED="${2:-${SEED:-1000}}"
KEY="${SSH_KEY:-$HOME/.ssh/id_rsa}"
HERE=$(CDPATH= cd "$(dirname "$0")" && pwd)
REPO=$(CDPATH= cd "$HERE/../.." && pwd)          # tests/tf → repo root
SSH="ssh -i $KEY -o StrictHostKeyChecking=accept-new admin@$IP"

echo ">> syncing version-locked tree ($REPO) → $IP"
rsync -az --delete -e "ssh -i $KEY -o StrictHostKeyChecking=accept-new" \
    --exclude '.git' --exclude 'target' --exclude 'tests/tf/.terraform' \
    --exclude 'tests/tf/*.tfstate*' \
    "$REPO/" "admin@$IP:zcc/"

echo ">> building zcc natively (aarch64 = the target ISA) + generating $SEED cases/suite + running the gate"
# Remote driver. Runs on the box; nproc there = the parallelism. Heredoc is quoted so $VARS
# below expand REMOTELY, except SEED which we bake in via the leading assignment.
$SSH "SEED=$SEED bash -s" <<'REMOTE'
set -eu
export PATH="$HOME/.cargo/bin:/opt/csmith/bin:/opt/yarpgen/bin:$PATH"
# The I/O storm (cargo target/, per-case mktemp compile/link/exec) goes to the local NVMe, not
# the 3000-IOPS EBS root. The source tree stays on EBS (small, read-mostly, page-cached).
export CARGO_TARGET_DIR=/mnt/nvme/target
export TMPDIR=/mnt/nvme/tmp
cd "$HOME/zcc"

# 1. Build zcc (native release, onto NVMe). No docker: this box IS aarch64 linux.
cargo build --release
ZCC="/mnt/nvme/target/release/zcc"; export ZCC
"$ZCC" --version 2>/dev/null || true

J=$(nproc)

# 2. Generate the csmith corpus (UB-free by construction) into /suites/csmith.
#    Standard `--seed N`; the include dir was seeded by cloud-init. `|| true` so one bad
#    seed cannot abort the whole run under `set -e` (xargs returns 123 on any child fail).
echo ">> generating $SEED csmith cases ($J jobs)"
seq 1 "$SEED" | xargs -P "$J" -n1 sh -c \
    'csmith --seed "$1" -o "/suites/csmith/c$1.c" >/dev/null 2>&1 || true' _

# 3. Generate the yarpgen corpus with the EXACT flags yarpgen.sh documents (its lines 26-30).
#    yarpgen writes driver.c/func.c/init.h INTO -o dir, which must EXIST first (unlike csmith's
#    single -o file) — mkdir -p per seed. stdout carries a /*SEED N*/ banner → /dev/null.
echo ">> generating $SEED yarpgen cases ($J jobs)"
seq 1 "$SEED" | xargs -P "$J" -n1 sh -c \
    'd="/suites/yarpgen/s$1"; mkdir -p "$d"; yarpgen --std=c --emit-align-attr=none --emit-pragmas=none --max-array-dims=3 -s "$1" -o "$d/" >/dev/null 2>&1 || true' _

# 4. Run both gates at full width. 0 DIVERGE is the invariant. tee the tallies to a log
#    (the pipe's exit is tee's 0, so csmith.sh's non-zero-on-DIVERGE cannot abort under set -e).
FAILDIR=/mnt/nvme/fails; rm -rf "$FAILDIR"; mkdir -p "$FAILDIR/csmith" "$FAILDIR/yarpgen"
echo "=== CSMITH $SEED ==="
CSMITH_JOBS="$J" sh tests/suites/csmith.sh "$SEED" 2>&1 | tee "$FAILDIR/csmith.log"
echo "=== YARPGEN $SEED ==="
YARP_JOBS="$J" sh tests/suites/yarpgen.sh "$SEED" 2>&1 | tee "$FAILDIR/yarpgen.log"

# 5. Bug-trace capture: pull the SOURCE of every DIVERGE case (the invariant breach) into
#    FAILDIR. csmith/yarpgen are seed-deterministic, so the source .c/dir IS the reproducer —
#    generator-version-independent, and (given zcc's HashMap nondeterminism) re-runnable N× locally.
#    The gate prints "DIVERGE: name(detail) ..."; parse the names, copy the sources.
for nm in $(sed -n 's/^DIVERGE://p' "$FAILDIR/csmith.log" | tr ' ' '\n' | sed 's/(.*//' | grep .); do
    cp "/suites/csmith/$nm.c" "$FAILDIR/csmith/" 2>/dev/null || true
done
for nm in $(sed -n 's/^DIVERGE://p' "$FAILDIR/yarpgen.log" | tr ' ' '\n' | sed 's/(.*//' | grep .); do
    cp -r "/suites/yarpgen/$nm" "$FAILDIR/yarpgen/" 2>/dev/null || true
done
# generator provenance (version-lock audit) + zcc binary hash (nondeterminism baseline)
{ echo "csmith: $(csmith --version 2>&1 | head -1)"; echo "yarpgen: $(yarpgen --version 2>&1 | head -1)";
  echo "zcc: $(sha256sum "$ZCC" | cut -c1-16)"; } > "$FAILDIR/provenance.txt"
echo ">> fail-capture: $(ls "$FAILDIR/csmith" | wc -l) csmith + $(ls "$FAILDIR/yarpgen" | wc -l) yarpgen DIVERGE sources in $FAILDIR"
REMOTE

# Pull the fail-capture back to the operator BEFORE any teardown — a paid divergence must be
# traceable offline without re-renting. Kept even on --destroy.
STAMP=$(date +%Y%m%d-%H%M%S)
LOCAL_FAILS="$HERE/fails-$STAMP"
echo ">> pulling fail-capture → $LOCAL_FAILS"
mkdir -p "$LOCAL_FAILS"
rsync -az -e "ssh -i $KEY -o StrictHostKeyChecking=accept-new" \
    "admin@$IP:/mnt/nvme/fails/" "$LOCAL_FAILS/" 2>/dev/null || true
echo ">> captured: $(find "$LOCAL_FAILS" -name '*.c' 2>/dev/null | wc -l | tr -d ' ') source files; logs in $LOCAL_FAILS"

if [ "$DESTROY" = 1 ]; then
    echo ">> --destroy: tearing down EVERYTHING"
    "$HERE/teardown.sh"
fi
