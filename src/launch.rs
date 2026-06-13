//! Lanzar Slay the Spire 2. El menu principal del juego ofrece "Load with Mods", asi que
//! alcanza con abrir el exe.

use crate::detect::Install;
use anyhow::{Context, Result};
use std::process::Command;

/// Lanza el juego desde su carpeta de install.
pub fn launch(install: &Install) -> Result<()> {
    let exe = install.root.join("SlayTheSpire2.exe");
    Command::new(&exe)
        .current_dir(&install.root)
        .spawn()
        .with_context(|| format!("no se pudo lanzar {}", exe.display()))?;
    Ok(())
}
