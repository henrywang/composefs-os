# cbootc

A minimal bootc-like tool for systems running on composefs-rs. Personal-scale,
distro-neutral, deliberately small.

## What This Is

cbootc deploys and updates Linux systems built as OCI container images, using
[composefs-rs](https://github.com/containers/composefs-rs) (via `cfsctl`) as
the storage backend. It is a thin operational layer above cfsctl: status,
upgrade, rollback, signature verification, systemd timer integration.

It is **not**:

- A general-purpose bootc replacement (use [bootc](https://github.com/bootc-dev/bootc))
- An ostree-compatible tool (it has no ostree code at all)
- A fleet management system

See [DESIGN.md](DESIGN.md) for the full design rationale and what's
deliberately out of scope.

## Status

Greenfield. Design done, no implementation yet. See DESIGN.md section
"Implementation Plan" for the order of work.

## Repository Layout

```
cbootc/
  DESIGN.md                  Design notes — start here
  README.md                  This file
  src/                       Rust source for the cbootc binary (not yet)
  tools/
    build-disk.sh            Build a qcow2/raw image from a container image
  examples/
    fedora/Containerfile     Minimal bootable Fedora image
    ubuntu/Containerfile     Minimal bootable Ubuntu image (TODO)
    arch/Containerfile       Minimal bootable Arch image (TODO)
```

## Quick Start (Once Implemented)

```sh
# Build a bootable image
podman build -t my-cfs-image:latest -f examples/fedora/Containerfile .

# Build a qcow2 disk image from it
sudo ./tools/build-disk.sh \
    containers-storage:my-cfs-image:latest \
    out.qcow2 \
    10

# Boot it
qemu-system-x86_64 -enable-kvm -m 4096 \
    -drive file=out.qcow2,if=virtio \
    -bios /usr/share/edk2/ovmf/OVMF_CODE.fd \
    -nographic

# Once running, upgrade in place:
sudo cbootc upgrade --reboot
```

## Building cbootc Itself

Not yet. First step: `cargo init` and the CLI skeleton from DESIGN.md
section "Implementation Plan" step 1.

## License

TBD. Probably MIT or Apache-2.0 to match the surrounding ecosystem.
