//! Config local de la app (rutas de la maquina, sets suscritos). Se guarda en
//! %APPDATA%/sts2-modsync/config.toml. NO contiene secretos (la clave PUBLICA de
//! firma va empotrada en el binario; la PRIVADA jamas toca al cliente).

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Version del esquema de la config local (subir si cambia de forma incompatible).
pub const CONFIG_SCHEMA: u32 = 1;

// Nota: serde ignora campos desconocidos, asi que abrir un config de una version FUTURA con
// una version vieja y volver a guardar PIERDE los campos nuevos (caso downgrade, poco comun).
// Los campos actuales se preservan (defaults + respaldo de corruptos en `load_from`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Version del esquema (para migraciones futuras; default = la actual).
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// Ruta del install de StS2 que el usuario fijo/confirmo (cachea la deteccion).
    #[serde(default)]
    pub install_root: Option<PathBuf>,
    /// URLs de manifiestos de set a los que el usuario esta suscripto.
    #[serde(default)]
    pub subscribed_sets: Vec<String>,
}

fn default_schema() -> u32 {
    CONFIG_SCHEMA
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: CONFIG_SCHEMA,
            install_root: None,
            subscribed_sets: Vec::new(),
        }
    }
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("io", "Chaldea", "sts2-modsync")
}

pub fn config_path() -> Option<PathBuf> {
    Some(project_dirs()?.config_dir().join("config.toml"))
}

pub fn load() -> Config {
    match config_path() {
        Some(p) => load_from(&p),
        None => Config::default(),
    }
}

/// Carga la config desde `path`. Si el archivo no existe -> default (1er arranque). Si existe
/// pero esta CORRUPTO -> NO resetea en silencio (perderia `install_root`/`subscribed_sets`):
/// respalda el invalido a `.toml.bad` (asi un `save` posterior no lo pisa) y avisa al log.
fn load_from(path: &Path) -> Config {
    let s = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Config::default(),
    };
    match toml::from_str::<Config>(&s) {
        Ok(cfg) => cfg,
        Err(e) => {
            crate::logging::log_line(&format!(
                "config invalida en {}: {e} — respaldada en .toml.bad, se usa una nueva",
                path.display()
            ));
            let bad = path.with_extension("toml.bad");
            let _ = std::fs::remove_file(&bad);
            let _ = std::fs::rename(path, &bad);
            Config::default()
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_from_respalda_config_corrupta_y_no_resetea_en_silencio() {
        let dir = std::env::temp_dir().join("sts2_modsync_cfg_corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "esto no es { toml valido =").unwrap();
        let cfg = load_from(&path);
        assert_eq!(cfg.schema, CONFIG_SCHEMA); // se usa default...
        assert!(path.with_extension("toml.bad").exists()); // ...pero el invalido se RESPALDO
        assert!(!path.exists()); // y no se pisa a ciegas
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_from_config_vieja_sin_schema_conserva_los_campos() {
        let dir = std::env::temp_dir().join("sts2_modsync_cfg_old");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        // config "vieja" (0.2.x) sin el campo `schema` -> serde le pone el default.
        std::fs::write(
            &path,
            "install_root = '/tmp/StS2'\nsubscribed_sets = ['https://x/s.json']\n",
        )
        .unwrap();
        let cfg = load_from(&path);
        assert_eq!(cfg.schema, CONFIG_SCHEMA);
        assert_eq!(cfg.install_root.as_deref(), Some(Path::new("/tmp/StS2")));
        assert_eq!(cfg.subscribed_sets, vec!["https://x/s.json".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_round_trip_toml() {
        let cfg = Config {
            schema: CONFIG_SCHEMA,
            install_root: Some(PathBuf::from("/tmp/StS2")),
            subscribed_sets: vec!["https://a/b.json".into()],
        };
        let back: Config = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(back.install_root, cfg.install_root);
        assert_eq!(back.subscribed_sets, cfg.subscribed_sets);
        assert_eq!(back.schema, CONFIG_SCHEMA);
    }
}
