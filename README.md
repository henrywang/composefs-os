# composefs-os

Bootable Linux system images built on [composefs-rs](https://github.com/containers/composefs-rs).
Image-based OS management for personal and small-scale systems — no ostree, no fleet tooling.

## What This Is

composefs-os publishes OCI container images that boot directly via a composefs
overlay filesystem. Each image ships `cbootc`, a small embedded tool that
handles upgrades, rollbacks, and image switching from within the running system.

It is **not**:

- A general-purpose [bootc](https://github.com/bootc-dev/bootc) replacement
- An ostree-compatible tool
- A fleet management system

See [DESIGN.md](DESIGN.md) for rationale and architecture.

## Available Images

| Image | Boot style | Status |
|-------|-----------|--------|
| `ghcr.io/henrywang/composefs-os:fedora-44` | GRUB (BLS Type 1) | Working |
| `ghcr.io/henrywang/composefs-os:fedora-44-uki` | systemd-boot + UKI (BLS Type 2) | Working |
| `ghcr.io/henrywang/composefs-os:fedora-44-uki-sb` | systemd-boot + UKI + Secure Boot | Working |
| Ubuntu | — | Planned |
| Arch Linux | — | Planned |


## Quick Start

```sh
# Install to a raw disk image (run from inside the container, needs --privileged)
sudo podman run --rm --privileged \
    -v $(pwd):/output \
    -v /var/lib/containers:/var/lib/containers \
    -v /var/tmp:/var/tmp \
    ghcr.io/henrywang/composefs-os:fedora-44 \
    cbootc install to-disk /output/disk.raw --size 10G

# Boot it
qemu-system-x86_64 -enable-kvm -m 4096 \
    -drive file=disk.raw,if=virtio \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.fd \
    -nographic
```

### Secure Boot

Pass `--secure-boot` to install the pre-signed Fedora shim + GRUB chain instead
of running `grub2-install`. The resulting image passes UEFI Secure Boot
enforcement without enrolling any custom keys.

```sh
# Install with Secure Boot EFI chain
sudo podman run --rm --privileged \
    -v $(pwd):/output \
    -v /var/lib/containers:/var/lib/containers \
    -v /var/tmp:/var/tmp \
    ghcr.io/henrywang/composefs-os:fedora-44 \
    cbootc install to-disk /output/disk-sb.raw --size 10G --secure-boot

# Boot with OVMF Secure Boot firmware (VARS must be a writable copy)
cp /usr/share/edk2/ovmf/OVMF_VARS.secboot.fd /tmp/OVMF_VARS.secboot.fd
qemu-system-x86_64 -enable-kvm -m 4096 \
    -machine q35,smm=on \
    -global driver=cfi.pflash01,property=secure,value=on \
    -drive file=disk-sb.raw,if=virtio \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd \
    -drive if=pflash,format=raw,file=/tmp/OVMF_VARS.secboot.fd \
    -nographic
```

The same GRUB base image works for both modes — the EFI chain difference is
handled entirely at install time.

### UKI (Unified Kernel Image)

The `-uki` image uses systemd-boot and BLS Type 2 entries: a single `.efi` file
bundles the kernel, initramfs, and `composefs=` cmdline. The cmdline (including
the composefs hash) is embedded at **install time** by `cbootc install to-disk`,
so there is no separate `.conf` file and no writable grubenv.

Pass `--uki` to `cbootc install to-disk` when using the UKI image:

```sh
sudo podman run --rm --privileged \
    -v $(pwd):/output \
    -v /var/lib/containers:/var/lib/containers \
    -v /var/tmp:/var/tmp \
    ghcr.io/henrywang/composefs-os:fedora-44-uki \
    cbootc install to-disk /output/disk-uki.raw --size 10G --uki

# A writable VARS file is needed for EFI variables (random seed, bootctl set-next)
cp /usr/share/edk2/ovmf/OVMF_VARS.fd /tmp/OVMF_VARS.fd
qemu-system-x86_64 -enable-kvm -m 4096 \
    -drive file=disk-uki.raw,if=virtio \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.fd \
    -drive if=pflash,format=raw,file=/tmp/OVMF_VARS.fd \
    -nographic
```

`cbootc rollback` automatically detects the boot style — it uses
`bootctl set-next` on UKI systems and `grub2-editenv` on GRUB systems.

### UKI + Secure Boot

The `-uki-sb` image combines UKI with Secure Boot enforcement. At install time
`cbootc` generates a self-signed key pair (or accepts `--sb-key`/`--sb-cert`),
signs both systemd-boot and the UKI `.efi`, and installs signed systemd-boot
directly as `BOOTx64.EFI` — no shim required. The firmware verifies the binaries
against its Signature Database (db).

The signing cert must be enrolled in the UEFI db once before Secure Boot
enforcement will allow booting. For QEMU testing, `prep_sb_vars.py` does this
automatically using `virt-fw-vars` (`dnf install python3-virt-firmware`):

```sh
# Install
sudo podman run --rm --privileged \
    -v $(pwd):/output \
    -v /var/lib/containers:/var/lib/containers \
    -v /var/tmp:/var/tmp \
    ghcr.io/henrywang/composefs-os:fedora-44-uki-sb \
    cbootc install to-disk /output/disk-uki-sb.raw --size 10G --uki --secure-boot
# disk-uki-sb.raw.sb.cer is written alongside the disk image

# Enroll the cert into a copy of OVMF_VARS
python3 tests/prep_sb_vars.py disk-uki-sb.raw ovmf-vars-uki-sb.fd

# Boot with Secure Boot enforcement
qemu-system-x86_64 -enable-kvm -m 4096 \
    -machine q35,smm=on \
    -global driver=cfi.pflash01,property=secure,value=on \
    -drive file=disk-uki-sb.raw,if=virtio \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd \
    -drive if=pflash,format=raw,file=ovmf-vars-uki-sb.fd \
    -nographic
```

On bare metal, enroll via the UEFI setup menu (**Secure Boot → Key Management →
Authorized Signatures → Add**) selecting `EFI/BOOT/composefs-os-sb.cer` from the
ESP. `cbootc upgrade` automatically re-signs new UKIs using the persisted key.

## Building a Custom Image

The published base images are a starting point. Add your own packages and
configuration in a derived `Containerfile`:

```dockerfile
FROM ghcr.io/henrywang/composefs-os:fedora-44

# Add packages
RUN dnf install -y vim htop && dnf clean all

# Bake in configuration that must survive upgrades
RUN echo 'myhost' > /etc/hostname
```

Use `examples/fedora/Containerfile` as a full template.

## In-System Management

Once booted, `cbootc` manages the system:

```sh
# Show current deployment status
cbootc status

# Pull the latest image and stage a new boot entry
cbootc upgrade

# Reboot to apply
systemctl reboot

# Roll back to the previous deployment
cbootc rollback
systemctl reboot

# Switch to a different image
cbootc switch docker://ghcr.io/henrywang/composefs-os:fedora-44
```

The tracked image reference is stored in `/var/lib/cbootc/config.toml` and
survives upgrades. `cbootc-update.timer` (enabled in the base image) runs
`cbootc upgrade` daily with a randomised delay.

## Repository Layout

```
composefs-os/
  Containerfile.base         Builds Fedora 44 base images (--target grub | uki | uki-secureboot)
  src/                       cbootc source (Rust)
  units/
    cbootc-update.service    Systemd service for automatic upgrades
    cbootc-update.timer      Systemd timer (daily, randomised delay)
  examples/
    fedora/
      Containerfile          Template for derived Fedora images
    arch/
      Containerfile          Arch Linux (stub — not yet functional)
    ubuntu/
      Containerfile          Ubuntu (stub — not yet functional)
  tests/
    e2e.py                   QEMU-based end-to-end test suite
  .github/workflows/
    ci.yml                   Rust build, test, lint
    container.yml            Build and push base image to ghcr.io
    e2e.yml                  End-to-end tests (boots in QEMU)
```

## Known Limitations

### /etc conflict resolution on upgrade

`cbootc upgrade` carries your `/etc` edits forward by copying the current
deployment's overlayfs upper directory into the new deployment's upper directory.
This gives the following behaviour:

- **File you edited, image didn't** → your version persists ✓
- **File image changed, you didn't** → new image version shows through ✓
- **Both you and the image changed the same file** → your version wins

The last case is the same default as bootc. If you'd rather an image update win
(e.g. after a security fix to a config file you've also customised), update your
local copy manually after upgrading.

**Tip:** for configuration that must be reproducible, bake it into the
`Containerfile` (e.g. `RUN echo 'myhost' > /etc/hostname`) rather than editing
the running system.

### Rollback

`cbootc rollback` selects the previous deployment for the next boot.
Run `systemctl reboot` to apply it.

- **GRUB systems**: writes `next_entry` to `/boot/grub2/grubenv`. If rollback
  fails to boot, use the GRUB menu to pick the older BLS entry manually from
  `/boot/loader/entries/`.
- **UKI/systemd-boot systems**: calls `bootctl set-next` to set the
  `LoaderEntryOneShot` EFI variable. If rollback fails to boot, use the
  systemd-boot menu (hold Space at startup) to pick the older `.efi` entry.

Old deployment boot files accumulate across upgrades and are not pruned
automatically. Remove them manually when disk space is a concern.

### x86-64 only

All boot paths (GRUB, UKI, UKI + Secure Boot) are hard-coded to `x86_64-efi`.
aarch64 and other architectures are not supported.

## License

MIT — see [LICENSE](LICENSE).
