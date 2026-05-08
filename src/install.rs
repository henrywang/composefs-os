use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::symlink;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Subcommand)]
pub enum InstallCommand {
    /// Partition a block device (or file) and install the current container image
    ToDisk(ToDiskOpts),
}

#[derive(Args)]
pub struct ToDiskOpts {
    /// Target block device (e.g. /dev/vda) or file path for loopback install
    device: PathBuf,
    /// Disk size for file-based loopback install (e.g. 10G); required when DEVICE is not a block device
    #[arg(long)]
    size: Option<String>,
    /// Root filesystem type
    #[arg(long, value_name = "TYPE", default_value = "ext4", value_parser = ["ext4", "xfs"])]
    filesystem: String,
    /// Run wipefs on the target before partitioning
    #[arg(long)]
    wipe: bool,
}

pub fn run(command: InstallCommand) -> Result<()> {
    match command {
        InstallCommand::ToDisk(opts) => run_to_disk(opts),
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn run_to_disk(opts: ToDiskOpts) -> Result<()> {
    check_root()?;
    let image_ref = detect_image_ref()?;
    println!("==> Source image: {image_ref}");

    let (dev, loop_dev) = prepare_device(&opts)?;

    let mnt = tempfile::TempDir::new().context("failed to create mount root")?;
    let mnt_path = mnt.path().to_path_buf();
    let mut mounts: Vec<PathBuf> = Vec::new();

    let result = install_inner(&dev, &image_ref, &opts.filesystem, &mnt_path, &mut mounts);

    // Always clean up, regardless of result
    for m in mounts.iter().rev() {
        let _ = Command::new("umount").arg(m).status();
    }
    if let Some(ref ld) = loop_dev {
        let _ = Command::new("losetup").arg("-d").arg(ld).status();
    }

    result?;

    println!("==> Done");
    if loop_dev.is_some() {
        let output = opts.device.display();
        println!();
        println!("Boot it with:");
        println!("  qemu-system-x86_64 -enable-kvm -m 4096 \\");
        println!("      -drive file={output},if=virtio \\");
        println!(
            "      -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.fd \\"
        );
        println!("      -nographic");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Device setup
// ---------------------------------------------------------------------------

fn prepare_device(opts: &ToDiskOpts) -> Result<(PathBuf, Option<String>)> {
    if opts.device.exists() && opts.device.metadata()?.file_type().is_block_device() {
        if opts.wipe {
            println!("==> Wiping {}", opts.device.display());
            run_cmd("wipefs", &["-a", opts.device.to_str().unwrap()])?;
        }
        return Ok((opts.device.clone(), None));
    }

    let size = opts
        .size
        .as_deref()
        .context("--size is required for file-based install (e.g. --size 10G)")?;
    println!("==> Creating {} image at {}", size, opts.device.display());
    run_cmd(
        "truncate",
        &[&format!("--size={size}"), opts.device.to_str().unwrap()],
    )?;
    println!("==> Attaching loop device");
    let ld = run_cmd_output(
        "losetup",
        &[
            "--find",
            "--show",
            "--partscan",
            opts.device.to_str().unwrap(),
        ],
    )?;
    println!("    {ld}");
    Ok((PathBuf::from(&ld), Some(ld)))
}

// ---------------------------------------------------------------------------
// Core installation
// ---------------------------------------------------------------------------

fn install_inner(
    dev: &Path,
    image_ref: &str,
    filesystem: &str,
    mnt_path: &Path,
    mounts: &mut Vec<PathBuf>,
) -> Result<()> {
    let dev_s = dev.to_str().unwrap();
    let efi_p = part(dev, 1);
    let boot_p = part(dev, 2);
    let root_p = part(dev, 3);

    println!("==> Partitioning {dev_s}");
    sfdisk_gpt(dev)?;
    // Update kernel's partition table view; udevadm settle handles bare-metal
    // but inside a container the host's udev never writes to the container's
    // private /dev.  ensure_partition_nodes() fills the gap via sysfs + mknod.
    let _ = Command::new("partx").args(["-u", dev_s]).status();
    let _ = Command::new("udevadm")
        .args(["settle", "--timeout=5"])
        .status();
    ensure_partition_nodes(dev, 3)?;

    println!("==> Formatting filesystems");
    run_cmd("mkfs.fat", &["-F32", "-n", "EFI", efi_p.to_str().unwrap()])?;
    run_cmd("mkfs.ext4", &["-F", "-L", "boot", boot_p.to_str().unwrap()])?;
    match filesystem {
        "xfs" => run_cmd("mkfs.xfs", &["-f", "-L", "root", root_p.to_str().unwrap()])?,
        _ => run_cmd(
            "mkfs.ext4",
            &["-F", "-L", "root", "-O", "verity", root_p.to_str().unwrap()],
        )?,
    }

    let root_uuid = blkid_uuid(root_p.to_str().unwrap())?;
    let boot_uuid = blkid_uuid(boot_p.to_str().unwrap())?;
    let efi_uuid = blkid_uuid(efi_p.to_str().unwrap())?;

    println!("==> Mounting");
    run_cmd(
        "mount",
        &[root_p.to_str().unwrap(), mnt_path.to_str().unwrap()],
    )?;
    mounts.push(mnt_path.to_path_buf());

    let boot_mnt = mnt_path.join("boot");
    fs::create_dir_all(&boot_mnt)?;
    run_cmd(
        "mount",
        &[boot_p.to_str().unwrap(), boot_mnt.to_str().unwrap()],
    )?;
    mounts.push(boot_mnt.clone());

    let efi_mnt = mnt_path.join("boot/efi");
    fs::create_dir_all(&efi_mnt)?;
    run_cmd(
        "mount",
        &[efi_p.to_str().unwrap(), efi_mnt.to_str().unwrap()],
    )?;
    mounts.push(efi_mnt.clone());

    // cfsctl (via containers-image) needs /var/tmp for blob staging.
    // The composefs image layout leaves /var empty; create the dir here.
    fs::create_dir_all("/var/tmp").context("create /var/tmp")?;

    println!("==> Initializing composefs repo");
    let cfs_repo = mnt_path.join("composefs");
    fs::create_dir_all(&cfs_repo)?;
    run_cmd("cfsctl", &["--repo", cfs_repo.to_str().unwrap(), "init"])?;

    println!("==> Pulling image: {image_ref}");
    run_cmd(
        "cfsctl",
        &[
            "--repo",
            cfs_repo.to_str().unwrap(),
            "oci",
            "pull",
            image_ref,
        ],
    )?;

    println!("==> Preparing boot entries");
    let digest = run_cmd_output(
        "cfsctl",
        &[
            "--repo",
            cfs_repo.to_str().unwrap(),
            "oci",
            "compute-id",
            "--bootable",
            image_ref,
        ],
    )?;
    let cmdline = format!("root=UUID={root_uuid} rootfstype={filesystem} rw console=ttyS0,115200");
    run_cmd(
        "cfsctl",
        &[
            "--repo",
            cfs_repo.to_str().unwrap(),
            "oci",
            "prepare-boot",
            "--bootdir",
            boot_mnt.to_str().unwrap(),
            "--entry-id",
            &digest,
            "--cmdline",
            cmdline.as_str(),
            image_ref,
        ],
    )?;
    crate::upgrade::patch_bls_entry(&boot_mnt, &digest, image_ref)?;

    // cfsctl oci prepare-boot creates state/deploy/<id>/etc/upper/ as the
    // overlayfs upperdir for /etc. Files placed there are visible in the
    // running system's /etc. The ext4 root's /etc is the lowerdir for the
    // composefs image's /etc, not for the overlay, so writing there has no
    // effect on what the booted system sees.
    let deploy_dir = find_deploy_dir(mnt_path)?;
    let etc_upper = deploy_dir.join("etc/upper");

    println!("==> Writing fstab");
    fs::write(
        etc_upper.join("fstab"),
        format!(
            "UUID={root_uuid}  /          {filesystem}  ro        0 1\n\
             UUID={boot_uuid}  /boot      ext4          defaults  0 2\n\
             UUID={efi_uuid}   /boot/efi  vfat          umask=0077,shortname=winnt  0 2\n"
        ),
    )?;

    // Replace the per-deployment var with a symlink to a shared state/var so
    // /var content (databases, logs, cbootc state) survives upgrades.
    fs::remove_dir(deploy_dir.join("var")).context("removing placeholder var dir")?;
    symlink("../../var", deploy_dir.join("var")).context("creating shared var symlink")?;

    println!("==> Installing GRUB");
    let grub = grub_install_bin();
    let efi_dir_arg = format!("--efi-directory={}", efi_mnt.display());
    let boot_dir_arg = format!("--boot-directory={}", boot_mnt.display());
    run_cmd(
        grub,
        &[
            "--target=x86_64-efi",
            efi_dir_arg.as_str(),
            boot_dir_arg.as_str(),
            "--bootloader-id=cbootc",
            "--removable",
            "--no-nvram",
            "--force",
        ],
    )?;

    println!("==> Writing grub.cfg");
    let grub2_dir = boot_mnt.join("grub2");
    fs::create_dir_all(&grub2_dir)?;
    fs::write(
        grub2_dir.join("grub.cfg"),
        "serial --unit=0 --speed=115200\n\
         terminal_input serial console\n\
         terminal_output serial console\n\
         load_env\n\
         if [ \"${next_entry}\" ] ; then\n\
           set default=\"${next_entry}\"\n\
           set next_entry=\n\
           save_env next_entry\n\
         fi\n\
         set timeout=3\n\
         insmod ext2\n\
         insmod all_video\n\
         function load_video { true; }\n\
         insmod blscfg\n\
         blscfg\n",
    )?;

    println!("==> Creating grubenv");
    let grubenv = grub2_dir.join("grubenv");
    grub_editenv_create(grubenv.to_str().unwrap())?;

    println!("==> Populating /var from image");
    let shared_var = mnt_path.join("state/var");
    fs::create_dir_all(&shared_var)?;
    run_cmd("cp", &["-ax", "/var/.", shared_var.to_str().unwrap()])?;

    println!("==> Writing cbootc config");
    fs::create_dir_all(shared_var.join("lib/cbootc"))?;
    fs::write(
        shared_var.join("lib/cbootc/config.toml"),
        format!("[image]\nref = \"{image_ref}\"\n"),
    )?;

    println!("==> Syncing");
    Command::new("sync")
        .status()
        .context("failed to run sync")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_deploy_dir(mnt_path: &Path) -> Result<PathBuf> {
    let state_deploy = mnt_path.join("state/deploy");
    let mut entries: Vec<_> = fs::read_dir(&state_deploy)
        .with_context(|| format!("reading {}", state_deploy.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    if entries.len() != 1 {
        bail!(
            "expected exactly one deployment in state/deploy, found {}",
            entries.len()
        );
    }
    Ok(entries.remove(0).path())
}

fn detect_image_ref() -> Result<String> {
    let content = fs::read_to_string("/run/.containerenv")
        .context("not running inside a container (no /run/.containerenv) — pass the image via cbootc switch first or run from within the container image")?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("image=") {
            let image = rest.trim_matches('"');
            if image.is_empty() {
                bail!("image= field in /run/.containerenv is empty");
            }
            return Ok(if image.contains("://") {
                image.to_string()
            } else {
                format!("containers-storage:{image}")
            });
        }
    }
    bail!("could not find image= field in /run/.containerenv")
}

fn check_root() -> Result<()> {
    let status = fs::read_to_string("/proc/self/status").context("read /proc/self/status")?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:\t") {
            let uid: u32 = rest
                .split_whitespace()
                .next()
                .unwrap_or("1")
                .parse()
                .unwrap_or(1);
            if uid != 0 {
                bail!("cbootc install to-disk must run as root");
            }
            return Ok(());
        }
    }
    bail!("could not determine UID from /proc/self/status")
}

fn part(dev: &Path, n: u8) -> PathBuf {
    let s = dev.to_str().unwrap();
    // Devices ending in a digit (e.g. nvme0n1, loop0) need a 'p' separator
    if s.chars()
        .last()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        PathBuf::from(format!("{s}p{n}"))
    } else {
        PathBuf::from(format!("{s}{n}"))
    }
}

// Inside a container the host udev never writes to the container's private /dev.
// sysfs IS global (all mounts see the same kernel state), so we can read the
// major:minor numbers from there and create the nodes ourselves with mknod.
fn ensure_partition_nodes(dev: &Path, count: u8) -> Result<()> {
    let dev_name = dev.file_name().unwrap().to_str().unwrap();
    for i in 1..=count {
        let part_dev = part(dev, i);
        if part_dev.exists() {
            continue;
        }
        let part_name = part_dev.file_name().unwrap().to_str().unwrap();
        let sys_dev = format!("/sys/block/{dev_name}/{part_name}/dev");
        let dev_nums = fs::read_to_string(&sys_dev).with_context(|| {
            format!("sysfs entry for {part_name} not found — partition table re-read may have failed")
        })?;
        let (major, minor) = dev_nums
            .trim()
            .split_once(':')
            .context("unexpected format in sysfs dev file")?;
        run_cmd("mknod", &[part_dev.to_str().unwrap(), "b", major, minor])?;
    }
    Ok(())
}

fn sfdisk_gpt(dev: &Path) -> Result<()> {
    let mut child = Command::new("sfdisk")
        .arg(dev)
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to spawn sfdisk")?;
    child.stdin.as_mut().unwrap().write_all(
        b"label: gpt\n\
          - : size=512MiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=\"EFI\"\n\
          - : size=1GiB,   type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=\"boot\"\n\
          - :               type=4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709, name=\"root\"\n",
    )?;
    let status = child.wait()?;
    if !status.success() {
        bail!("sfdisk failed with {status}");
    }
    Ok(())
}

fn blkid_uuid(dev: &str) -> Result<String> {
    run_cmd_output("blkid", &["-s", "UUID", "-o", "value", dev])
}

fn grub_install_bin() -> &'static str {
    if Command::new("grub2-install")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        "grub2-install"
    } else {
        "grub-install"
    }
}

fn grub_editenv_create(path: &str) -> Result<()> {
    for cmd in &["grub2-editenv", "grub-editenv"] {
        match Command::new(cmd).args([path, "create"]).status() {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => bail!("{cmd} create: exited {s}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("spawning {cmd}")),
        }
    }
    bail!("neither grub2-editenv nor grub-editenv found in PATH")
}

fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to run {program}"))?;
    if !status.success() {
        bail!("{program} failed with {status}");
    }
    Ok(())
}

fn run_cmd_output(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    if !out.status.success() {
        bail!("{program} failed with {}", out.status);
    }
    Ok(String::from_utf8(out.stdout)
        .context("non-UTF-8 output")?
        .trim()
        .to_string())
}
