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
  python3 tests/e2e.py --secure-boot disk-sb.raw
"""

import argparse
import os
import re
import shutil
import sys
import tempfile
import pexpect

PROMPT = r"\[root@[^\]]+\]#"
TIMEOUT_BOOT = 180
TIMEOUT_CMD = 30

OVMF_CANDIDATES = [
    "/usr/share/edk2/ovmf/OVMF_CODE.fd",       # Fedora
    "/usr/share/OVMF/OVMF_CODE.fd",             # Ubuntu ≤23.10
    "/usr/share/OVMF/OVMF_CODE_4M.fd",          # Ubuntu 24.04+
    "/usr/share/ovmf/OVMF_CODE.fd",
]

OVMF_SECBOOT_CODE_CANDIDATES = [
    "/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd",   # Fedora
    "/usr/share/OVMF/OVMF_CODE_4M.ms.fd",           # Ubuntu 24.04
    "/usr/share/OVMF/OVMF_CODE.secboot.fd",         # Ubuntu older
]

OVMF_SECBOOT_VARS_CANDIDATES = [
    "/usr/share/edk2/ovmf/OVMF_VARS.secboot.fd",   # Fedora
    "/usr/share/OVMF/OVMF_VARS_4M.ms.fd",           # Ubuntu 24.04
    "/usr/share/OVMF/OVMF_VARS.secboot.fd",         # Ubuntu older
]


def find_ovmf():
    for path in OVMF_CANDIDATES:
        if os.path.exists(path):
            return path
    return None


def find_ovmf_secboot():
    code = next((p for p in OVMF_SECBOOT_CODE_CANDIDATES if os.path.exists(p)), None)
    vars_ = next((p for p in OVMF_SECBOOT_VARS_CANDIDATES if os.path.exists(p)), None)
    return code, vars_


def boot(disk_image, ovmf_code, ovmf_vars=None):
    if ovmf_vars:
        # Secure Boot OVMF requires q35 + SMM for the firmware's variable-locking
        # code; without smm=on the firmware stalls silently before any serial output.
        machine = "-machine q35,smm=on -global driver=cfi.pflash01,property=secure,value=on"
        pflash = (
            f"-drive if=pflash,format=raw,readonly=on,file={ovmf_code} "
            f"-drive if=pflash,format=raw,file={ovmf_vars}"
        )
    else:
        machine = ""
        pflash = f"-drive if=pflash,format=raw,readonly=on,file={ovmf_code}"
    cmd = (
        f"qemu-system-x86_64 -enable-kvm -m 2048 "
        f"{machine} "
        f"-drive file={disk_image},if=virtio,snapshot=on "
        f"{pflash} "
        f"-nographic -no-reboot"
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

def test_secure_boot_enabled(child):
    """SecureBoot EFI variable must report enabled (last attribute byte = 1)."""
    rc, out = run_cmd(
        child,
        "od -An -t u1 "
        "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c "
        "2>/dev/null | tr -s ' ' '\\n' | tail -1"
    )
    assert rc == 0, "SecureBoot EFI variable not readable"
    assert out.strip() == "1", f"Secure Boot not active (last byte = {out.strip()!r})"


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
    parser.add_argument("--ovmf", help="Path to OVMF CODE flash file")
    parser.add_argument("--ovmf-vars", help="Path to OVMF VARS flash file (Secure Boot mode)")
    parser.add_argument(
        "--secure-boot",
        action="store_true",
        help="Boot with Secure Boot enforcement (requires OVMF secboot firmware)",
    )
    args = parser.parse_args()

    ovmf_vars_tmp = None
    try:
        if args.secure_boot:
            sb_code, sb_vars = find_ovmf_secboot()
            ovmf_code = args.ovmf or sb_code
            ovmf_vars_src = args.ovmf_vars or sb_vars
            if not ovmf_code or not ovmf_vars_src:
                print(
                    "ERROR: Secure Boot OVMF firmware not found. "
                    "Install edk2-ovmf (Fedora) or ovmf (Ubuntu), "
                    "or pass --ovmf / --ovmf-vars."
                )
                sys.exit(1)
            fd, ovmf_vars_tmp = tempfile.mkstemp(suffix=".fd")
            os.close(fd)
            shutil.copy2(ovmf_vars_src, ovmf_vars_tmp)
            tests = [test_secure_boot_enabled] + TESTS
        else:
            ovmf_code = args.ovmf or find_ovmf()
            if not ovmf_code:
                print("ERROR: OVMF firmware not found. Install edk2-ovmf or pass --ovmf.")
                sys.exit(1)
            tests = TESTS

        print(f"Booting {args.disk_image} ...")
        child = boot(args.disk_image, ovmf_code, ovmf_vars_tmp)

        try:
            wait_for_shell(child)
            print("==> Boot OK\n")

            passed = failed = 0
            for test_fn in tests:
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
    finally:
        if ovmf_vars_tmp and os.path.exists(ovmf_vars_tmp):
            os.unlink(ovmf_vars_tmp)


if __name__ == "__main__":
    main()
