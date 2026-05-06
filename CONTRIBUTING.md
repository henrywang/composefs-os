# Contributing

## Prerequisites

- Rust stable (`rustup toolchain install stable`)
- For integration tests: `podman`, `qemu-system-x86_64`, OVMF firmware

## Build

```sh
cargo build --release
# binary: target/release/cbootc
```

## Lint

```sh
cargo fmt --check
cargo clippy -- -D warnings
```

## Building the example image

```sh
# Base image (slow — runs dnf + dracut inside the container)
podman build -t fedora-cfs-base:43 -f examples/fedora/Containerfile.base .

# Derived image (fast)
podman build -t my-fedora-cfs:latest -f examples/fedora/Containerfile .
```

## Testing install to-disk

```sh
# Loopback install (creates a raw disk image from inside the container)
podman run --rm --privileged \
  -v $(pwd):/output \
  my-fedora-cfs:latest \
  cbootc install to-disk /output/disk.raw --size 10G

qemu-system-x86_64 -enable-kvm -m 4096 \
  -drive file=disk.raw,if=virtio \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.fd \
  -nographic
```

## Pull Requests

- One logical change per PR.
- `cargo fmt` and `cargo clippy` must pass.
- Update `tests/INTEGRATION.md` if behaviour changes.
