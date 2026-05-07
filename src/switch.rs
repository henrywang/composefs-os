use anyhow::Result;

use crate::{config, upgrade};

pub fn run(image_ref: String) -> Result<()> {
    let image_ref = upgrade::normalize_ref(&image_ref);
    config::write_image_ref(&image_ref)?;
    println!("Tracking {image_ref}.");
    upgrade::run(false)
}
