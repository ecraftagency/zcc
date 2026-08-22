#!/bin/bash
# cloud-init.sh — runs once as root on first boot. Installs ONLY the toolchain + the two
# fuzz generators, so the box is fuzz-ready. It does NOT fetch zcc or run anything: the
# version-locked source arrives via run-fuzz.sh (rsync of the working tree = the lock).
#
# Distro = Debian 13 (trixie) arm64 — the EXACT environment of the local zcc-box (same gcc 14.2,
# same glibc at /usr/lib/aarch64-linux-gnu that zcc's driver hardcodes). aarch64 IS zcc's native
# target (Article F): zcc builds and runs here directly, NO docker (the Mac needs the zcc-box
# container only because darwin is not aarch64-linux; here it is native). Default user = "admin".
set -eux
exec > /var/log/zcc-cloud-init.log 2>&1

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get -y install build-essential cmake m4 python3 git rsync xfsprogs \
    curl ca-certificates zlib1g-dev libssl-dev

# --- Local NVMe instance store = the fuzz working disk (leverage its ~1000x IOPS headroom) ---
# c8gd exposes ephemeral NVMe with MODEL "Amazon EC2 NVMe Instance Storage", distinct from the
# EBS root ("Amazon Elastic Block Store"). UNFORMATTED + wiped on terminate ⟹ mkfs fresh each boot
# (the box is always created fresh). ALL compile/link/exec churn (cargo target/, corpus, TMPDIR)
# lives here, keeping the 3000-IOPS EBS root idle. Provisioning 40k IOPS on EBS would cost ~10x the
# box (io2 ~$3.6/hr) — the NVMe is free and faster. The MODEL string is hardware, distro-independent.
NVME=$(lsblk -dpno NAME,MODEL | awk '/Instance Storage/{print $1; exit}')
[ -n "$NVME" ] || { echo "FATAL: no instance-store NVMe (is this a c*gd box?)" >&2; exit 1; }
mkfs.xfs -f -L nvme "$NVME"
mkdir -p /mnt/nvme
mount -o noatime,nodiratime "$NVME" /mnt/nvme          # noatime: no read->write amplification
mkdir -p /mnt/nvme/target /mnt/nvme/suites/csmith/include /mnt/nvme/suites/yarpgen /mnt/nvme/tmp
chown -R admin:admin /mnt/nvme
# /suites -> NVMe: the suite scripts (csmith.sh DIR=/suites/csmith, yarpgen.sh /suites/yarpgen)
# stay byte-identical to the local box — the harness never knows it is on NVMe.
ln -sfn /mnt/nvme/suites /suites

# --- Rust toolchain (for building zcc), installed for the admin user ---
sudo -u admin bash -lc '
  curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
'

# --- csmith (differential program generator) → /opt/csmith ---
git clone --depth 1 https://github.com/csmith-project/csmith.git /tmp/csmith
cmake -S /tmp/csmith -B /tmp/csmith/build -DCMAKE_INSTALL_PREFIX=/opt/csmith
cmake --build /tmp/csmith/build -j "$(nproc)"
cmake --install /tmp/csmith/build

# --- yarpgen (differential program generator) → /opt/yarpgen ---
git clone --depth 1 https://github.com/intel/yarpgen.git /tmp/yarpgen
cmake -S /tmp/yarpgen -B /tmp/yarpgen/build
cmake --build /tmp/yarpgen/build -j "$(nproc)"
install -Dm755 /tmp/yarpgen/build/yarpgen /opt/yarpgen/bin/yarpgen

# --- csmith runtime headers → /suites/csmith/include (csmith.sh sets INC there) ---
# csmith.h #includes random_inc.h + others, so copy the WHOLE header dir, not just csmith.h
# (copying only csmith.h was the "random_inc.h: No such file" gcc-SKIP bug the calibration caught).
csmith_inc=$(dirname "$(find /opt/csmith/include -name csmith.h | head -1)")
cp "$csmith_inc"/*.h /suites/csmith/include/
chown -R admin:admin /suites/

# PATH for the generators, for interactive + run-fuzz sessions
echo 'export PATH=/opt/csmith/bin:/opt/yarpgen/bin:$PATH' > /etc/profile.d/zcc-fuzz.sh

echo "zcc fuzz box ready" > /var/log/zcc-ready
