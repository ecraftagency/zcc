# main.tf — a single spot Graviton box for the milestone csmith/yarpgen gate (10k seeds).
# Provisions infra ONLY: the box, its toolchain (via cloud-init), a locked-down SSH SG.
# Deploy + run is run-fuzz.sh (rsync the version-locked tree up, build, generate, run).
#
# Lifecycle at a milestone:  terraform apply  →  ./run-fuzz.sh  →  read verdict  →  terraform destroy
# It is a BATCH box: spot one-time, so it is never auto-restarted; destroy it when done.

terraform {
  required_version = ">= 1.6"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = var.region
  default_tags {
    tags = merge({
      Project   = "zcc"
      Purpose   = "milestone-fuzz-gate"
      ManagedBy = "terraform"
    }, var.tags)
  }
}

# Debian 13 (trixie) arm64 — the EXACT distro of the local zcc-box (Debian 13, gcc 14.2,
# glibc at /usr/lib/aarch64-linux-gnu). This is a Law-1 requirement, not a preference: zcc's
# driver hardcodes Debian multiarch linker paths (src/main.rs:319,328,352), and the differential
# referee is gcc — so the STRONG gate must run the SAME environment as the weak gate, or it
# measures distro drift, not the compiler. (AL2023 put crt in /usr/lib64 → zcc could not link.)
# Owner 136693071363 = Debian's official AWS account. Default login user on Debian AMIs = "admin".
data "aws_ami" "debian13_arm64" {
  most_recent = true
  owners      = ["136693071363"]

  filter {
    name   = "name"
    values = ["debian-13-arm64-*"]
  }
  filter {
    name   = "architecture"
    values = ["arm64"]
  }
  filter {
    name   = "virtualization-type"
    values = ["hvm"]
  }
}

# Upload the operator's local public key (default ~/.ssh/id_rsa.pub). Fresh name ⟹ no clash
# with the region's existing default key pair; the matching local private key still signs in.
resource "aws_key_pair" "fuzz" {
  key_name_prefix = "zcc-fuzz-"
  public_key      = file(pathexpand(var.public_key_path))
}

resource "aws_security_group" "fuzz" {
  name_prefix = "zcc-fuzz-"
  description = "SSH from the operator only; all egress (git/toolchain fetch)."

  ingress {
    description = "SSH"
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = [var.ssh_cidr]
  }

  egress {
    description = "all egress"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_instance" "fuzz" {
  ami                    = data.aws_ami.debian13_arm64.id
  instance_type          = var.instance_type
  key_name               = aws_key_pair.fuzz.key_name
  vpc_security_group_ids = [aws_security_group.fuzz.id]

  # Spot, one-time: a fuzz gate is a batch job — run to completion, do not resurrect.
  instance_market_options {
    market_type = "spot"
    spot_options {
      spot_instance_type             = "one-time"
      max_price                      = var.spot_max_price != "" ? var.spot_max_price : null
      instance_interruption_behavior = "terminate"
    }
  }

  # Nothing survives teardown: the root volume dies with the instance. Spot one-time instances
  # cannot "stop" (only terminate), and instance_initiated_shutdown_behavior is unsettable on
  # spot (AWS rejects the modify) — so a self-issued `shutdown -h` terminates by default; no
  # attribute needed. terraform destroy is the primary teardown; teardown.sh sweeps any stragglers.
  root_block_device {
    volume_type           = "gp3"
    volume_size           = var.root_volume_gb
    encrypted             = true
    delete_on_termination = true
  }

  # cloud-init installs the toolchain + generators so the box is fuzz-ready on boot.
  # Plain file() (not templatefile) so bash ${VAR} never collides with TF interpolation;
  # run-fuzz.sh carries the seed count and drives the run.
  user_data = file("${path.module}/cloud-init.sh")

  tags = { Name = "zcc-fuzz-${var.instance_type}" }
}
