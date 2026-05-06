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

Steps 1–7 of the implementation plan are complete. The binary compiles and all
five commands are wired up. Step 8 (integration tests in QEMU) is next.

## Repository Layout

```
cbootc/
  DESIGN.md                  Design notes — start here
  README.md                  This file
  src/                       Rust source for the cbootc binary
  units/
    cbootc-update.service    Systemd service for automatic upgrades
    cbootc-update.timer      Systemd timer (daily, with randomized delay)
  tools/
    build-disk.sh            Build a qcow2/raw image from a container image
  examples/
    fedora/Containerfile     Minimal bootable Fedora image
    ubuntu/Containerfile     Minimal bootable Ubuntu image (TODO)
    arch/Containerfile       Minimal bootable Arch image (TODO)
```

## Quick Start

```sh
# Build a bootable image (also compiles cbootc inside the build)
podman build -t my-cfs-image:latest -f examples/fedora/Containerfile .

# Build a qcow2 disk image from it
sudo ./tools/build-disk.sh \
    containers-storage:my-cfs-image:latest \
    out.qcow2 \
    10

# Boot it
qemu-system-x86_64 -enable-kvm -m 4096 \
    -drive file=out.qcow2,if=virtio \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.fd \
    -nographic

# Once running, upgrade in place:
sudo cbootc upgrade --reboot
```

## Automatic Updates

The example Containerfiles install and enable the update timer automatically.
On a running system, or if you built a custom image, install manually:

```sh
sudo cp units/cbootc-update.{service,timer} /etc/systemd/system/
sudo systemctl enable --now cbootc-update.timer
```

The timer runs `cbootc upgrade` once a day with a randomised one-hour delay.
It stages the new image but does not reboot — reboot on your own schedule.
The service is a no-op when `/etc/cbootc/config.toml` does not exist.

## Building cbootc Itself

```sh
cargo build --release
# binary at target/release/cbootc
```

## License

TBD. Probably MIT or Apache-2.0 to match the surrounding ecosystem.
