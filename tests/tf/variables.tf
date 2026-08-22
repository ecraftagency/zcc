# variables.tf — the milestone fuzz-gate box. Every default carries provenance
# (Article E numeric-provenance): it is either a spec/measured number or has a stated
# premise. No magic numbers.

variable "region" {
  description = <<-EOT
    AWS region. us-east-2 (Ohio) — cheapest c7g.8xlarge spot (measured 2026-08:
    $0.342/hr vs $0.398 us-east-1, $0.424 us-west-2, $0.627 ap-southeast-1). The batch
    needs no network proximity, and the key pair is uploaded fresh per-region, so cost is
    the only criterion.
  EOT
  type    = string
  default = "us-east-2"
}

variable "instance_type" {
  description = <<-EOT
    Graviton (aarch64) instance. Locked to c8gd.16xlarge by Law-1 derivation:
      - aarch64 = zcc's ONLY target ISA (Article F: AArch64-ELF Linux). Graviton runs the
        exact target natively — no emulation, no docker shim (the Mac needs zcc-box only
        because darwin is not aarch64-linux; here it IS native).
      - c8g = Graviton4 (~+30%% per-core vs c7g/G3 — the wall-time lever, since fuzzing is
        compile-bound). 16xlarge = 64 vCPU / 128 GiB / 3800GB NVMe.
      - ".d" = local NVMe instance store. The per-case churn (mktemp→compile→link→exec) is a
        small-IOPS storm; default gp3 root is 3000 IOPS (size-independent), and PROVISIONING
        40k IOPS on EBS costs ~10x the box (io2 ~$3.6/hr vs spot ~$0.59/hr). The NVMe ships
        FREE, gives ~1000x the IOPS headroom, and is ephemeral (self-cleaning on terminate).
        cloud-init mounts it at /mnt/nvme; run-fuzz points target/, corpus, TMPDIR there.
      - 64 vCPU (not 32): the 1000 CALIBRATION run must measure the SAME hardware the 10000
        seal runs on (faithful t̄ — 64-way memory/NVMe contention included). Spot quirk: 16xl
        is cheaper PER CORE than 8xl ($0.586/64 < $0.348/32), so 64c wins on cost AND wall.
        The harness reads nproc; nothing else changes with size.
  EOT
  type        = string
  default     = "c8gd.16xlarge"
}

# Seed count is NOT a Terraform var — it is a run-fuzz.sh argument (default 1000), so a box
# can be re-fuzzed at 1000 then 10000 without reprovisioning.

variable "root_volume_gb" {
  description = <<-EOT
    Root gp3 size. The root holds ONLY the OS + toolchain (rustup ~2GB + gcc + csmith/yarpgen
    build ~1GB + OS ~3GB ≈ 6-8GB used) — the corpus, cargo target/, and TMPDIR all live on the
    ephemeral NVMe (/mnt/nvme), off EBS. So 30GB is comfortable. gp3 IOPS is size-independent
    (3000 baseline regardless of GB), and the root is near-idle once scratch is on NVMe, so
    there is no perf reason to oversize — bigger disk = pure waste here.
  EOT
  type        = number
  default     = 30
}

variable "public_key_path" {
  description = <<-EOT
    Local SSH PUBLIC key to authorize on the box. Defaults to the operator's ~/.ssh/id_rsa.pub,
    so the matching ~/.ssh/id_rsa signs in with no extra flags. TF uploads only the public half;
    the private key never enters TF or state. A fresh key-pair name is used, so it does not
    collide with the region's existing default key pair.
  EOT
  type        = string
  default     = "~/.ssh/id_rsa.pub"
}

variable "ssh_cidr" {
  description = <<-EOT
    CIDR allowed to SSH (port 22). No default ON PURPOSE — an open 0.0.0.0/0 default is a
    security smell this project does not ship. Pass your own IP, e.g. -var 'ssh_cidr=1.2.3.4/32'
    (get it with: curl -s ifconfig.me).
  EOT
  type        = string

  validation {
    condition     = can(cidrhost(var.ssh_cidr, 0))
    error_message = "ssh_cidr must be a valid CIDR, e.g. 203.0.113.4/32."
  }
}

variable "spot_max_price" {
  description = "Max spot $/hr. Empty string ⟹ no cap (pay up to the on-demand price — the safe default that avoids capacity-starvation)."
  type        = string
  default     = ""
}

variable "tags" {
  description = "Extra tags merged onto every resource (cost tracking / ownership)."
  type        = map(string)
  default     = {}
}
