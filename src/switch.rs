use anyhow::Result;

use crate::{config, upgrade};

pub fn run(image_ref: String) -> Result<()> {
    config::write_image_ref(&image_ref)?;
    println!("Tracking {image_ref}.");
    upgrade::run(false)
}
