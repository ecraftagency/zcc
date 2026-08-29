# variables.tf — the milestone fuzz-gate box. Every default carries provenance
# (Article E numeric-provenance): it is either a spec/measured number or has a stated
# premise. No magic numbers.

variable "region" {
  description = <<-EOT
    us-west-2 (Oregon), and this is an OPERATOR RULE with no exceptions, not a cost
    optimization: **every AWS resource this project creates lives in us-west-2.**
    One region means one place to audit and one place to sweep, and the incident
    that motivates it is on record — an unqualified CLI call (the AWS default is
    whatever the profile says, NOT this variable) reached a production account in
    another region. Never pass a different region to `terraform apply`, and always
    pass `--region us-west-2` to a bare `aws` call.

    The earlier cost note is kept because the number is still true and someone will
    otherwise re-derive it: us-east-2 was measured cheapest for c7g.8xlarge spot in
    2026-08 ($0.342/hr vs $0.398 us-east-1, $0.424 us-west-2, $0.627 ap-southeast-1).
    The rule outranks the saving.

    AND THE AZ SPREAD INSIDE ONE REGION IS WIDER THAN THE REGION SPREAD, measured
    2026-08-29 for c8gd.2xlarge in us-west-2: 2b $0.0650, 2c $0.0694, 2a $0.0911,
    2d $0.1375 — a 2.1x range. Nothing here pins an AZ, so a run lands wherever the
    default subnet puts it (the 2026-08-29 benchmark box landed in 2a, 40% over 2b).
    Pin the cheap AZ with a subnet_id if the box is long-lived; under an hour the
    difference is a few cents and not worth the coupling.
  EOT
  type    = string
  default = "us-west-2"
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
    TWO SIZES, ONE FAMILY — and the family is the part that must not change, because
    cloud-init's NVMe mount and every `/suites` path key off the ".d" instance store.
    Only the size moves:

      - `c8gd.8xlarge` (32 vCPU) — the csmith/yarpgen 10k seal. Operator-set size.
        A per-core cost argument favours 16xl on paper (spot $0.586/64 < $0.348/32),
        and it is recorded here so nobody re-derives it as if it were new; the
        operator sets 8xl anyway, and a rule beats a paper saving.
      - `c8gd.2xlarge` (8 vCPU) — the sqlite / suite EXEC measurement. This use wants
        a QUIET core, not many of them: it is a second microarchitecture (Graviton4 =
        Neoverse V2) standing beside the M1 Pro, and every extra vCPU is idle cost.
        Measured 2026-08-29, us-west-2a: total $0.0944/hr = $0.00157/min (spot
        $0.0911 + gp3 30GB $0.00329). zcc builds native there in 8.5 s — no external
        crates, so a release build is one rustc invocation.

    SPOT IS RECLAIMED, AND ON A MEASUREMENT RUN THAT COSTS MORE THAN THE SAVING.
    Measured 2026-08-29: a c8gd.2xlarge one-time spot box in us-west-2a was
    terminated by AWS ("Service initiated") after ~15 minutes, mid-session. For the
    fuzz seal that is survivable — the batch is restartable and the verdict is the
    only artefact. For BENCHMARKING it is not free: an interrupted interleaved pair
    is not half a measurement, it is no measurement. Write the remote work as ONE
    batch that prints each result as it finishes, so a reclaim costs the tail and
    not the run. On-demand is ~2x spot and removes this entirely; the operator has
    chosen spot, and this note is here so the choice stays informed.

    THE NVMe IS EPHEMERAL AND THAT IS THE POINT. It costs nothing because it is
    instance store: it is wiped on stop and gone on terminate, so ANY result left
    only on /mnt/nvme is lost with the box. Pull verdicts and artefacts back over
    ssh before `terraform destroy` — run-fuzz.sh's rsync-back exists for exactly
    this. What it buys is ~1000x the IOPS headroom of the 3000-IOPS gp3 root, free;
    provisioning that on EBS (io2 ~$3.6/hr) would cost 38x the whole box.
  EOT
  type        = string
  default     = "c8gd.8xlarge"
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
