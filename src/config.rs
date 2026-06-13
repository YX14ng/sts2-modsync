//! Config local de la app (rutas de la maquina, sets suscritos). Se guarda en
//! %APPDATA%/sts2-modsync/config.toml. NO contiene secretos (la clave PUBLICA de
//! firma va empotrada en el binario; la PRIVADA jamas toca al cliente).

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Ruta del install de StS2 que el usuario fijo/confirmo (cachea la deteccion).
    #[serde(default)]
    pub install_root: Option<PathBuf>,
    /// URLs de manifiestos de set a los que el usuario esta suscripto.
    #[serde(default)]
    pub subscribed_sets: Vec<String>,
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("io", "Chaldea", "sts2-modsync")
}

pub fn config_path() -> Option<PathBuf> {
    Some(project_dirs()?.config_dir().join("config.toml"))
}

pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path().context("no se pudo resolver el directorio de config")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let s = toml::to_string_pretty(cfg)?;
    std::fs::write(&path, s).with_context(|| format!("escribiendo {}", path.display()))?;
    Ok(())
}
