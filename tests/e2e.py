#!/usr/bin/env python3
"""
End-to-end tests for composefs-os.

Boots a pre-built disk image in QEMU and verifies cbootc behaviour over the
serial console.  Requires:
  - qemu-system-x86_64
  - OVMF firmware (edk2-ovmf or ovmf package)
  - pexpect  (pip install pexpect)
  - A disk image built from examples/fedora/Containerfile (has serial autologin)

Usage:
  python3 tests/e2e.py disk.raw
  python3 tests/e2e.py --ovmf /path/to/OVMF_CODE.fd disk.raw
"""

import argparse
import re
import sys
import pexpect

PROMPT = r"\[root@[^\]]+\]#"
TIMEOUT_BOOT = 180
TIMEOUT_CMD = 30

OVMF_CANDIDATES = [
    "/usr/share/edk2/ovmf/OVMF_CODE.fd",
    "/usr/share/OVMF/OVMF_CODE.fd",
    "/usr/share/ovmf/OVMF_CODE.fd",
]


def find_ovmf():
    import os
    for path in OVMF_CANDIDATES:
        if os.path.exists(path):
            return path
    return None


def boot(disk_image, ovmf):
    cmd = (
        f"qemu-system-x86_64 -enable-kvm -m 2048 "
        f"-drive file={disk_image},if=virtio,snapshot=on "
        f"-drive if=pflash,format=raw,readonly=on,file={ovmf} "
        f"-serial stdio -nographic -no-reboot"
    )
    child = pexpect.spawn(cmd, timeout=TIMEOUT_BOOT, encoding="utf-8")
    child.logfile_read = sys.stdout
    return child


def wait_for_shell(child):
    child.expect(PROMPT, timeout=TIMEOUT_BOOT)


def run_cmd(child, cmd):
    """Run a shell command; return (exit_code, output_before_exit_marker)."""
    marker = "__E2E_EXIT__"
    child.sendline(f"{cmd}; echo {marker}$?")
    child.expect(rf"{marker}(\d+)", timeout=TIMEOUT_CMD)
    exit_code = int(child.match.group(1))
    output = child.before
    child.expect(PROMPT, timeout=TIMEOUT_CMD)
    return exit_code, output


# ---------------------------------------------------------------------------
# Individual tests
# ---------------------------------------------------------------------------

def test_status(child):
    """cbootc status: Digest populated, Image configured, no error trace."""
    rc, out = run_cmd(child, "cbootc status")
    assert rc == 0, f"cbootc status exited {rc}"
    assert re.search(r"Digest:\s+\S", out), f"Digest field missing or empty:\n{out}"
    assert "(not booted from composefs)" not in out, "not running from composefs"
    assert "Image:" in out, f"Image field missing:\n{out}"


def test_bls_title(child):
    """BLS entry title must not be the cfsctl placeholder 'todoOS'."""
    rc, out = run_cmd(child, "grep '^title ' /boot/loader/entries/*.conf")
    assert rc == 0, "no BLS entries found"
    assert "todoOS" not in out, f"BLS title still 'todoOS':\n{out}"


def test_rollback_no_previous(child):
    """cbootc rollback with a single BLS entry must fail with a clear message."""
    rc, out = run_cmd(child, "cbootc rollback")
    assert rc != 0, "expected non-zero exit when no previous deployment exists"
    assert "no previous" in out.lower(), f"unexpected error output:\n{out}"


def test_grubenv_exists(child):
    """/boot/grub2/grubenv must exist (written at install time)."""
    rc, _ = run_cmd(child, "test -f /boot/grub2/grubenv")
    assert rc == 0, "/boot/grub2/grubenv not found"


def test_var_config(child):
    """/var/lib/cbootc/config.toml must exist and contain the image ref."""
    rc, out = run_cmd(child, "cat /var/lib/cbootc/config.toml")
    assert rc == 0, "config.toml not found"
    assert "ref" in out, f"image ref missing from config.toml:\n{out}"


# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

TESTS = [
    test_status,
    test_bls_title,
    test_rollback_no_previous,
    test_grubenv_exists,
    test_var_config,
]


def main():
    parser = argparse.ArgumentParser(description="composefs-os e2e tests")
    parser.add_argument("disk_image", help="Path to disk.raw")
    parser.add_argument("--ovmf", help="Path to OVMF_CODE.fd")
    args = parser.parse_args()

    ovmf = args.ovmf or find_ovmf()
    if not ovmf:
        print("ERROR: OVMF firmware not found. Install edk2-ovmf or pass --ovmf.")
        sys.exit(1)

    print(f"Booting {args.disk_image} ...")
    child = boot(args.disk_image, ovmf)

    try:
        wait_for_shell(child)
        print("==> Boot OK\n")

        passed = failed = 0
        for test_fn in TESTS:
            name = test_fn.__name__
            try:
                test_fn(child)
                print(f"  PASS  {name}")
                passed += 1
            except AssertionError as e:
                print(f"  FAIL  {name}: {e}")
                failed += 1

        print(f"\n{passed} passed, {failed} failed")
        sys.exit(0 if failed == 0 else 1)

    except pexpect.TIMEOUT:
        print("\nFAIL: timed out waiting for output")
        print("Last output:")
        print(child.before)
        sys.exit(1)
    finally:
        child.sendline("poweroff -f")
        child.expect(pexpect.EOF, timeout=30)


if __name__ == "__main__":
    main()
