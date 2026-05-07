# cbootc

A minimal bootc-like tool for systems running on composefs-rs. Personal-scale,
distro-neutral, deliberately small.

## What This Is

cbootc deploys and updates Linux systems built as OCI container images, using
[composefs-rs](https://github.com/containers/composefs-rs) (via `cfsctl`) as
the storage backend. It is a thin operational layer above cfsctl: install,
status, upgrade, switch, rollback, signature verification, and a systemd timer
for automatic updates.

It is **not**:

- A general-purpose bootc replacement (use [bootc](https://github.com/bootc-dev/bootc))
- An ostree-compatible tool (no ostree code)
- A fleet management system

See [DESIGN.md](DESIGN.md) for full design rationale.

## Quick Start

```sh
# Build the base image (slow — dnf + dracut inside the container)
podman build -t fedora-cfs-base:43 -f examples/fedora/Containerfile.base .

# Build a derived image (fast)
podman build -t my-fedora-cfs:latest -f examples/fedora/Containerfile .

# Install to a raw disk image (run from inside the container, needs --privileged)
sudo podman run --rm --privileged \
    -v $(pwd):/output \
    -v /var/lib/containers:/var/lib/containers \
    -v /var/tmp:/var/tmp \
    my-fedora-cfs:latest \
    cbootc install to-disk /output/disk.raw --size 10G

# Boot it
qemu-system-x86_64 -enable-kvm -m 4096 \
    -drive file=disk.raw,if=virtio \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.fd \
    -nographic
```

## Upgrading a Running System

```sh
# Point the system at a registry image
cbootc switch docker://ghcr.io/you/my-fedora-cfs:latest

# Pull latest and stage new boot entry
cbootc upgrade

# Reboot to apply
systemctl reboot
```

The image reference is persisted in `/var/lib/cbootc/config.toml` and survives
upgrades. The `cbootc-update.timer` (enabled in the base image) runs
`cbootc upgrade` daily with a randomised delay.

## Repository Layout

```
cbootc/
  src/                       Rust source
  units/
    cbootc-update.service    Systemd service for automatic upgrades
    cbootc-update.timer      Systemd timer (daily, randomised delay)
  examples/
    fedora/
      Containerfile.base     Builds the bootable Fedora 43 base image
      Containerfile          Example derived image (your customisations go here)
```

## Known Limitations

### /etc changes are lost on upgrade

When `cbootc upgrade` stages a new image, the new deployment gets a fresh
`/etc` overlay upper directory copied from the new image. Any files you edited
in `/etc` on the running system (e.g. `/etc/hostname`, `/etc/ssh/sshd_config`)
are in the **old** deployment's overlay and are not carried forward.

[bootc](https://github.com/bootc-dev/bootc) solves this with a three-way
`etc-merge`: it diffs the old image's `/etc` against the running `/etc` (to
find your local edits), then re-applies those edits onto the new image's `/etc`.
cbootc does not implement this yet.

**Workaround:** manage `/etc` changes via the container image. Put configuration
in the `Containerfile` (e.g. `RUN echo 'myhost' > /etc/hostname`) so it is
part of the image and survives every upgrade automatically.

### No rollback support yet

`cbootc rollback` is stubbed but not implemented. If an upgrade fails to boot,
use the GRUB menu to select the previous BLS boot entry manually.

### x86-64 only

The GRUB install step (`grub2-install --target=x86_64-efi`) is hard-coded to
x86-64 EFI. aarch64 and other architectures are not supported.

## License

MIT — see [LICENSE](LICENSE) (if present) or SPDX identifier in source files.
