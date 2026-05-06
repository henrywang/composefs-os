use clap::{Parser, Subcommand};

mod cfsctl;
mod config;
mod install;
mod rollback;
mod signing;
mod status;
mod switch;
mod upgrade;
mod verify;

#[derive(Parser)]
#[command(name = "cbootc", about = "Minimal bootc-like tool for composefs-rs systems", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show current digest, tracked image, and last upgrade time
    Status,
    /// Pull latest image and prepare boot entry
    Upgrade {
        /// Reboot immediately after a successful upgrade
        #[arg(long)]
        reboot: bool,
    },
    /// Mark previous deployment as next boot
    Rollback,
    /// Change tracked image reference
    Switch {
        /// Image reference to switch to (e.g. docker://registry/image:tag)
        image_ref: String,
    },
    /// Verify current image's signature against configured key
    Verify,
    /// Install the current container image onto a disk
    Install {
        #[command(subcommand)]
        command: install::InstallCommand,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Status => status::run(),
        Command::Upgrade { reboot } => upgrade::run(reboot),
        Command::Rollback => rollback::run(),
        Command::Switch { image_ref } => switch::run(image_ref),
        Command::Verify => verify::run(),
        Command::Install { command } => install::run(command),
    }
}
