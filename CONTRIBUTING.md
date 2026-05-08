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
podman build -t composefs-os:fedora-43 -f Containerfile.base .

# Derived image (fast — no special steps required)
podman build -t my-fedora-cfs:latest -f examples/fedora/Containerfile .
```

Custom images need no `cfs-layout-apply` or `FROM scratch` step. The pattern is:

```dockerfile
FROM composefs-os:fedora-43
RUN dnf install -y myapp && dnf clean all
LABEL containers.bootc=1
CMD ["/sbin/init"]
```

## Testing install to-disk

`cbootc install to-disk` must run inside the container image with:
- `--privileged` — for disk and mknod access
- `-v /var/lib/containers:/var/lib/containers` — so cfsctl/skopeo can read the image from container storage
- `-v /dev:/dev` — for physical disk installs; not needed for loopback

```sh
# Loopback install (creates a raw disk image from inside the container)
sudo podman run --rm --privileged \
  -v $(pwd):/output \
  -v /var/lib/containers:/var/lib/containers \
  -v /var/tmp:/var/tmp \
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
