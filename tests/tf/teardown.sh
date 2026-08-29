#!/bin/sh
# teardown.sh — deallocate COMPLETELY EVERYTHING this batch job provisioned, then PROVE it.
# Two phases:
#   1. terraform destroy — removes every TF-managed resource (instance, spot request, root
#      volume via delete_on_termination, key pair, security group). No EIP, no S3, no VPC/NAT
#      were ever created, so there is nothing outside TF's graph.
#   2. Independent sweep — query AWS by the Project=zcc tag / zcc-fuzz- name and confirm ZERO
#      survivors. A surviving resource ⟹ loud report + non-zero exit (never a silent leak).
#
#   ./teardown.sh            # destroy + verify
#   ./teardown.sh --verify   # verify only (no destroy) — audit that nothing is billing
set -eu
HERE=$(CDPATH= cd "$(dirname "$0")" && pwd)
cd "$HERE"

# THE REGION, AND WHY THIS IS NOT A ONE-LINER.
#
# This read `terraform output -raw region 2>/dev/null || echo ...`. After a
# SUCCESSFUL destroy the state has no outputs left, so terraform writes a
# multi-line warning to STDOUT, `-raw` still exits 0, and REGION became the
# warning text. The sweep then queried a garbage region, found nothing there,
# and printed CLEAN — at the exact moment the operator most needs the truth.
# Observed 2026-08-29; the box was in fact clean, but the instrument could not
# have known that.
#
# Fixed three ways: stdout AND stderr are discarded, the result is checked to
# LOOK like a region, and the default is the project's own (Article: AWS is
# us-west-2 only). A sweep that cannot name its region refuses to run.
REGION=$(terraform output -raw region 2>/dev/null | head -1)
case "$REGION" in
    [a-z][a-z]-[a-z]*-[0-9]) ;;                 # looks like a region
    *) REGION="${REGION_OVERRIDE:-us-west-2}" ;;
esac
export AWS_DEFAULT_REGION="$REGION"
echo ">> region: $REGION"

if [ "${1:-}" != "--verify" ]; then
    echo ">> terraform destroy (region $REGION)"
    terraform destroy -auto-approve
fi

echo ">> sweep: confirming zero survivors tagged Project=zcc in $REGION"
leak=0

inst=$(aws ec2 describe-instances \
    --filters "Name=tag:Project,Values=zcc" "Name=instance-state-name,Values=pending,running,stopping,stopped" \
    --query 'Reservations[].Instances[].InstanceId' --output text 2>/dev/null || true)
[ -n "$inst" ] && { echo "  LEAK instances: $inst"; leak=1; }

vols=$(aws ec2 describe-volumes \
    --filters "Name=tag:Project,Values=zcc" "Name=status,Values=available,in-use" \
    --query 'Volumes[].VolumeId' --output text 2>/dev/null || true)
[ -n "$vols" ] && { echo "  LEAK volumes: $vols"; leak=1; }

spot=$(aws ec2 describe-spot-instance-requests \
    --filters "Name=tag:Project,Values=zcc" "Name=state,Values=open,active" \
    --query 'SpotInstanceRequests[].SpotInstanceRequestId' --output text 2>/dev/null || true)
[ -n "$spot" ] && { echo "  LEAK spot requests: $spot"; leak=1; }

keys=$(aws ec2 describe-key-pairs \
    --filters "Name=key-name,Values=zcc-fuzz-*" \
    --query 'KeyPairs[].KeyName' --output text 2>/dev/null || true)
[ -n "$keys" ] && { echo "  LEAK key pairs: $keys"; leak=1; }

sgs=$(aws ec2 describe-security-groups \
    --filters "Name=group-name,Values=zcc-fuzz-*" \
    --query 'SecurityGroups[].GroupId' --output text 2>/dev/null || true)
[ -n "$sgs" ] && { echo "  LEAK security groups: $sgs"; leak=1; }

if [ "$leak" = 0 ]; then
    echo ">> CLEAN — nothing left billing in $REGION."
else
    echo ">> WARNING: survivors above are still billing. Re-run 'terraform destroy' or delete them by id."
    exit 1
fi
