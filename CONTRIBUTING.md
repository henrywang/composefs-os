# Contributing

## Prerequisites

- Rust stable (`rustup toolchain install stable`)
- `just` — `sudo apt install just` (Ubuntu 24.04+) or see https://just.systems
- For container and e2e tests: `podman`, `qemu-system-x86_64`, OVMF firmware (`edk2-ovmf` on Fedora, `ovmf` on Ubuntu)

## Rust

```sh
just build    # compile  →  target/release/cbootc
just test     # unit tests
just check    # fmt + clippy, no writes (run this before pushing)
just fmt      # reformat source
```

## Building container images

```sh
# Fedora base images (slow — runs dnf + dracut inside the container)
just build-base       # GRUB/shim boot
just build-base-uki   # systemd-boot + UKI

# Fedora example images layered on top (fast)
just build-example        # GRUB example  →  composefs-os-test:latest
just build-example-uki    # UKI example   →  composefs-os-uki-test:latest

# Ubuntu base images (slow — runs apt + dracut inside the container)
just build-base-ubuntu              # GRUB
just build-base-ubuntu-uki          # systemd-boot + UKI
just build-base-ubuntu-uki-secureboot  # UKI + Secure Boot

# Ubuntu example images layered on top (fast)
just build-example-ubuntu        # GRUB example  →  composefs-os-ubuntu-test:latest
just build-example-ubuntu-uki    # UKI example   →  composefs-os-ubuntu-uki-test:latest
```

Custom images follow the same pattern — no `FROM scratch` or layout step needed:

```dockerfile
FROM composefs-os:fedora-44
RUN dnf install -y myapp && dnf clean all
LABEL containers.bootc=1
CMD ["/sbin/init"]
```

## Install to disk and run e2e tests

`cbootc install to-disk` runs inside the container with `--privileged` for device access.
Pass `-v /dev:/dev` for physical disk installs; not needed for the loopback images below.

```sh
# Create disk images (requires sudo) — Fedora
just install-disk              # GRUB           →  disk.raw
just install-disk-secureboot   # Secure Boot    →  disk-sb.raw
just install-disk-uki          # UKI            →  disk-uki.raw

# Run e2e tests against those images — Fedora
just e2e                       # GRUB tests
just e2e-secureboot            # Secure Boot tests
just e2e-uki                   # UKI tests
```

Or use the all-in-one recipes that build, install, and test in one shot:

```sh
# Fedora
just ci-grub        # GRUB end-to-end
just ci-secureboot  # Secure Boot end-to-end
just ci-uki         # UKI end-to-end

# Ubuntu
just ci-ubuntu-grub           # GRUB end-to-end
just ci-ubuntu-uki            # UKI end-to-end
just ci-ubuntu-uki-secureboot # UKI + Secure Boot end-to-end
```

## Pull Requests

- One logical change per PR.
- `just check` must pass (fmt + clippy).
- Update `tests/INTEGRATION.md` if behaviour changes.
