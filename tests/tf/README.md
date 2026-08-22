# Milestone fuzz-gate — Graviton spot box (`tests/tf/`)

The correctness gate has **two tiers**:

| tier | when | where | scale |
|---|---|---|---|
| **weak** | minor iteration | local Mac docker box (`box.sh`), capped jobs | csmith/yarpgen **300** seeds |
| **strong** | milestone iteration, **version-locked** | this: an AWS Graviton **spot** box | csmith/yarpgen **10,000** seeds |

The weak gate is fast and keeps your laptop cool. The strong gate is the expensive
random-differential proof (0 DIVERGE over 10k UB-free programs per generator) you run when
locking a version — on rented cores, not your laptop.

Why Graviton: `aarch64` is zcc's *only* target ISA (Article F). A Graviton box runs the exact
target **natively** — no emulation, no docker shim (the Mac needs `zcc-box` only because darwin
isn't aarch64-linux; here it is). gcc, the differential referee, ships in Amazon Linux 2023.

## Use it

```sh
cd tests/tf
terraform init
terraform apply -var 'ssh_cidr=<your-ip>/32'      # get your ip: curl -s ifconfig.me
terraform output ready_check                        # wait: cloud-init installs the toolchain
#   → ssh admin@<ip> 'cloud-init status --wait'

./run-fuzz.sh <public-ip>                           # SEED=10000 default; rsyncs the version-locked tree
#   or a smaller sweep:  SEED=2000 ./run-fuzz.sh <ip>

./teardown.sh                                       # destroy EVERYTHING + verify zero survivors
```

One-shot (run, then auto-destroy on completion):

```sh
./run-fuzz.sh --destroy <public-ip>
```

## Deallocation — completely everything

`teardown.sh` runs `terraform destroy` **and then independently sweeps** AWS by the `Project=zcc`
tag / `zcc-fuzz-*` names, failing loudly if any instance, volume, spot request, key pair, or
security group survives. The footprint is deliberately minimal so there is nothing to leak:

- **spot, one-time** + `interruption_behavior=terminate` + `instance_initiated_shutdown_behavior=terminate`
  → the box can never linger stopped-but-billing.
- root volume `delete_on_termination=true` → dies with the instance.
- **no** Elastic IP, **no** S3, **no** new VPC/NAT — uses the default VPC and a free auto-assigned
  public IP that is released on terminate.

Audit anytime without destroying: `./teardown.sh --verify`.

## Cost note

Spot `c7g.8xlarge` (32 vCPU) in ap-southeast-1 is on the order of a few tenths of a USD/hour; a
10k×2 sweep is well under an hour on 32 cores (generation + compile+run). Bump to `c7g.16xlarge`
(64 vCPU) to roughly halve wall-clock — the harness reads `nproc`, nothing else changes. Always
`teardown.sh` when done.

## Files

- `main.tf` — provider, AL2023-arm64 AMI, spot instance, SSH-only security group, key pair from
  your `~/.ssh/id_rsa.pub`.
- `variables.tf` — every default carries provenance (region, instance type, seed count, volumes).
- `cloud-init.sh` — first-boot toolchain: rust, gcc referee, csmith + yarpgen built from source,
  `/suites/{csmith,yarpgen}` case dirs.
- `run-fuzz.sh` — rsync the version-locked tree → build zcc → generate N cases → run both gates.
- `teardown.sh` — destroy + verify-zero-survivors.
