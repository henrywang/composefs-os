#!/usr/bin/env bash
# Upload a composefs-os raw disk image to S3 and register it as an AWS AMI.
#
# Prerequisites:
#   - aws CLI configured with appropriate credentials and region
#   - S3 bucket exists in the target region
#   - vmimport IAM role exists with S3 and EC2 permissions
#     (see https://docs.aws.amazon.com/vm-import/latest/userguide/required-permissions.html)
#
# Usage:
#   ./examples/upload-ami.sh -d disk-aws-fedora.raw -b my-bucket
#   ./examples/upload-ami.sh -d disk-aws-ubuntu.raw -b my-bucket -n my-ami -r us-west-2

set -euo pipefail

AMI_NAME="composefs-os-$(date +%Y%m%d-%H%M%S)"
DISK_IMAGE=""
S3_BUCKET=""
REGION=""
KEEP_S3=false

usage() {
    cat <<'EOF'
Usage: upload-ami.sh -d DISK -b BUCKET [-n NAME] [-r REGION] [-k]

  -d DISK     Raw disk image path (e.g. disk-aws-fedora.raw)
  -b BUCKET   S3 bucket name for staging the image
  -n NAME     AMI name (default: composefs-os-TIMESTAMP)
  -r REGION   AWS region (default: AWS_DEFAULT_REGION or CLI config)
  -k          Keep the S3 object after import (default: delete it)
EOF
    exit 1
}

while getopts "d:b:n:r:kh" opt; do
    case $opt in
        d) DISK_IMAGE="$OPTARG" ;;
        b) S3_BUCKET="$OPTARG" ;;
        n) AMI_NAME="$OPTARG" ;;
        r) REGION="$OPTARG" ;;
        k) KEEP_S3=true ;;
        *) usage ;;
    esac
done

[[ -z "$DISK_IMAGE" || -z "$S3_BUCKET" ]] && usage
[[ -f "$DISK_IMAGE" ]] || { echo "error: disk image not found: $DISK_IMAGE"; exit 1; }

AWS=(aws)
[[ -n "$REGION" ]] && AWS+=(--region "$REGION")

S3_KEY=$(basename "$DISK_IMAGE")

echo "==> Uploading $DISK_IMAGE to s3://$S3_BUCKET/$S3_KEY"
"${AWS[@]}" s3 cp "$DISK_IMAGE" "s3://$S3_BUCKET/$S3_KEY"

echo "==> Starting snapshot import"
TASK_ID=$("${AWS[@]}" ec2 import-snapshot \
    --disk-container "Format=RAW,UserBucket={S3Bucket=$S3_BUCKET,S3Key=$S3_KEY}" \
    --query 'ImportTaskId' --output text)
echo "    Task: $TASK_ID"

echo "==> Waiting for import to complete (may take several minutes)"
while true; do
    DETAIL=$("${AWS[@]}" ec2 describe-import-snapshot-tasks \
        --import-task-ids "$TASK_ID" \
        --query 'ImportSnapshotTasks[0].SnapshotTaskDetail' \
        --output json)
    STATUS=$(echo "$DETAIL" | grep -o '"Status": "[^"]*"' | cut -d'"' -f4)
    PROGRESS=$(echo "$DETAIL" | grep -o '"Progress": "[^"]*"' | cut -d'"' -f4 || true)
    echo "    Status: $STATUS${PROGRESS:+ (${PROGRESS}%)}"
    case "$STATUS" in
        completed) break ;;
        deleted|deleting)
            MSG=$(echo "$DETAIL" | grep -o '"StatusMessage": "[^"]*"' | cut -d'"' -f4 || true)
            echo "error: import failed${MSG:+: $MSG}"
            exit 1 ;;
    esac
    sleep 15
done

SNAPSHOT_ID=$("${AWS[@]}" ec2 describe-import-snapshot-tasks \
    --import-task-ids "$TASK_ID" \
    --query 'ImportSnapshotTasks[0].SnapshotTaskDetail.SnapshotId' \
    --output text)
echo "    Snapshot: $SNAPSHOT_ID"

if ! $KEEP_S3; then
    echo "==> Deleting s3://$S3_BUCKET/$S3_KEY"
    "${AWS[@]}" s3 rm "s3://$S3_BUCKET/$S3_KEY"
fi

echo "==> Registering AMI: $AMI_NAME"
AMI_ID=$("${AWS[@]}" ec2 register-image \
    --name "$AMI_NAME" \
    --architecture x86_64 \
    --virtualization-type hvm \
    --boot-mode uefi \
    --ena-support \
    --root-device-name /dev/xvda \
    --block-device-mappings "[{\"DeviceName\":\"/dev/xvda\",\"Ebs\":{\"SnapshotId\":\"$SNAPSHOT_ID\",\"VolumeType\":\"gp3\",\"DeleteOnTermination\":true}}]" \
    --query 'ImageId' --output text)

echo "==> Done: $AMI_ID"
