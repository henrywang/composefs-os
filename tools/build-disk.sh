#!/bin/bash
# build-disk.sh — Deploy a composefs-rs container image into a qcow2/raw disk image.
#
# Usage: sudo ./build-disk.sh <image-ref> <output> [size_gb]
# Example:
#   sudo ./build-disk.sh containers-storage:fedora-cfs-minimal:latest out.qcow2 10
#
# Image-ref accepts anything cfsctl pull accepts:
#   - containers-storage:<image>:<tag>
#   - docker://registry/image:tag
#   - oci-archive:/path/to/file.oci

set -euo pipefail

IMAGE_REF="${1:?Usage: $0 <image-ref> <output> [size_gb]}"
OUTPUT="${2:?Usage: $0 <image-ref> <output> [size_gb]}"
SIZE_GB="${3:-10}"

WORKDIR=$(mktemp -d -t cfs-build-XXXXXX)
RAW_IMAGE="$WORKDIR/disk.raw"
MOUNT_ROOT="$WORKDIR/mnt"
LOOP_DEV=""
mkdir -p "$MOUNT_ROOT"

cleanup() {
    set +e
    if mountpoint -q "$MOUNT_ROOT/boot/efi"; then umount "$MOUNT_ROOT/boot/efi"; fi
    if mountpoint -q "$MOUNT_ROOT/boot";     then umount "$MOUNT_ROOT/boot"; fi
    if mountpoint -q "$MOUNT_ROOT";          then umount "$MOUNT_ROOT"; fi
    if [ -n "$LOOP_DEV" ]; then losetup -d "$LOOP_DEV" 2>/dev/null; fi
    rm -rf "$WORKDIR"
}
trap cleanup EXIT

require() {
    command -v "$1" >/dev/null 2>&1 || { echo "Missing: $1" >&2; exit 1; }
}

require qemu-img
require sgdisk
require mkfs.ext4
require mkfs.fat
require losetup
require partprobe
require cfsctl
command -v grub2-install >/dev/null 2>&1 || command -v grub-install >/dev/null 2>&1 \
    || { echo "Missing: grub2-install or grub-install" >&2; exit 1; }

echo "==> Creating raw image (${SIZE_GB} GB)"
truncate -s "${SIZE_GB}G" "$RAW_IMAGE"

echo "==> Partitioning"
sgdisk --zap-all "$RAW_IMAGE"
sgdisk --new=1:0:+512M --typecode=1:ef00 --change-name=1:EFI    "$RAW_IMAGE"
sgdisk --new=2:0:+1G   --typecode=2:8300 --change-name=2:boot   "$RAW_IMAGE"
sgdisk --new=3:0:0     --typecode=3:8300 --change-name=3:root   "$RAW_IMAGE"

echo "==> Attaching loop device"
LOOP_DEV=$(losetup --find --show --partscan "$RAW_IMAGE")
partprobe "$LOOP_DEV"
sleep 1

EFI_PART="${LOOP_DEV}p1"
BOOT_PART="${LOOP_DEV}p2"
ROOT_PART="${LOOP_DEV}p3"

echo "==> Formatting filesystems"
mkfs.fat -F32 -n EFI    "$EFI_PART"  >/dev/null
mkfs.ext4 -F -L boot    "$BOOT_PART" >/dev/null
mkfs.ext4 -F -L root -O verity "$ROOT_PART" >/dev/null
ROOT_UUID=$(blkid -s UUID -o value "$ROOT_PART")

echo "==> Mounting"
mount "$ROOT_PART" "$MOUNT_ROOT"
mkdir -p "$MOUNT_ROOT/boot"
mount "$BOOT_PART" "$MOUNT_ROOT/boot"
mkdir -p "$MOUNT_ROOT/boot/efi"
mount "$EFI_PART" "$MOUNT_ROOT/boot/efi"

echo "==> Initializing composefs repo"
mkdir -p "$MOUNT_ROOT/composefs"
cfsctl --repo "$MOUNT_ROOT/composefs" init

echo "==> Pulling image: $IMAGE_REF"
cfsctl --repo "$MOUNT_ROOT/composefs" oci pull "$IMAGE_REF"

echo "==> Computing image digest"
DIGEST=$(cfsctl --repo "$MOUNT_ROOT/composefs" oci compute-id --bootable "$IMAGE_REF")
echo "    digest = $DIGEST"

echo "==> Preparing boot entries"
cfsctl --repo "$MOUNT_ROOT/composefs" oci prepare-boot \
    --bootdir "$MOUNT_ROOT/boot" \
    --cmdline "root=UUID=$ROOT_UUID rootfstype=ext4 rw console=ttyS0,115200" \
    "$IMAGE_REF"

echo "==> Setting up /var and /etc mountpoints"
mkdir -p "$MOUNT_ROOT/var" "$MOUNT_ROOT/etc"

echo "==> Writing fstab"
BOOT_UUID=$(blkid -s UUID -o value "$BOOT_PART")
EFI_UUID=$(blkid -s UUID -o value "$EFI_PART")
cat > "$MOUNT_ROOT/etc/fstab" <<EOF
UUID=$ROOT_UUID  /          ext4  defaults  0 1
UUID=$BOOT_UUID  /boot      ext4  defaults  0 2
UUID=$EFI_UUID   /boot/efi  vfat  umask=0077,shortname=winnt  0 2
EOF

echo "==> Installing GRUB (UEFI, removable path)"
GRUB_INSTALL=$(command -v grub2-install || command -v grub-install)
"$GRUB_INSTALL" \
    --target=x86_64-efi \
    --efi-directory="$MOUNT_ROOT/boot/efi" \
    --boot-directory="$MOUNT_ROOT/boot" \
    --bootloader-id=cbootc \
    --removable \
    --no-nvram \
    --force

# Minimal grub.cfg that just scans BLS entries cfsctl wrote
mkdir -p "$MOUNT_ROOT/boot/grub2"
cat > "$MOUNT_ROOT/boot/grub2/grub.cfg" <<'EOF'
set timeout=3
serial --unit=0 --speed=115200
terminal_input serial console
terminal_output serial console
insmod ext2
insmod all_video
function load_video { true; }
insmod blscfg
blscfg
EOF

echo "==> Syncing and unmounting"
sync
umount "$MOUNT_ROOT/boot/efi"
umount "$MOUNT_ROOT/boot"
umount "$MOUNT_ROOT"
losetup -d "$LOOP_DEV"
LOOP_DEV=""

echo "==> Writing output"
case "$OUTPUT" in
    *.qcow2)
        qemu-img convert -f raw -O qcow2 -c "$RAW_IMAGE" "$OUTPUT"
        ;;
    *.raw|*.img)
        mv "$RAW_IMAGE" "$OUTPUT"
        ;;
    *)
        echo "Unknown extension, defaulting to qcow2"
        qemu-img convert -f raw -O qcow2 -c "$RAW_IMAGE" "$OUTPUT"
        ;;
esac

echo "==> Done: $OUTPUT"
echo
echo "Boot it with:"
echo "  qemu-system-x86_64 -enable-kvm -m 4096 \\"
echo "      -drive file=$OUTPUT,if=virtio \\"
echo "      -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.fd \\"
echo "      -nographic"
