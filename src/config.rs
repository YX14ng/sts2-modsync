//! Config local de la app (rutas de la maquina, sets suscritos). Se guarda en
//! %APPDATA%/sts2-modsync/config.toml. NO contiene secretos (la clave PUBLICA de
//! firma va empotrada en el binario; la PRIVADA jamas toca al cliente).

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    /// Ultima version sincronizada por set (url -> set_version), para marcar "version nueva".
    #[serde(default)]
    pub set_versions: HashMap<String, String>,
    /// "owner/repo" del ULTIMO repo donde el modder publico: se reusa para que "actualizar" sea
    /// subir otro RELEASE al mismo repo (no crear repos nuevos cada vez).
    #[serde(default)]
    pub publish_repo: Option<String>,
    /// Ultimo nombre de set publicado (para pre-cargar el form de Publicar).
    #[serde(default)]
    pub publish_set_name: Option<String>,
    /// Origen de cada mod para auto-actualizar (id del mod -> `ModSource::to_storage()`, ej
    /// "github:owner/repo" o "nexus:game/id"). Lo pega el usuario; tiene prioridad sobre el origen
    /// declarado en el `<id>.json` del mod.
    #[serde(default)]
    pub mod_sources: HashMap<String, String>,
    /// Canal GLOBAL de actualizacion: `true` = seguir versiones BETA (pre-releases en GitHub),
    /// `false` = solo estables (MAIN). Default estable.
    #[serde(default)]
    pub prefer_beta: bool,
    /// Ultimo TAG de release que `mod-update` instalo por mod (id -> tag). Sirve para no re-ofrecer
    /// la MISMA version a un mod que no declara `version` en su `<id>.json` (sino seria un loop).
    #[serde(default)]
    pub mod_installed_tag: HashMap<String, String>,
    /// Lanzar el build de Steam POR Steam (`steam://rungameid/<appid>`): overlay, horas e
    /// invitaciones. `false` = abrir el exe directo (deja `steam_appid.txt` para que SteamAPI
    /// inicialice). Solo aplica a builds de Steam; las copias pirata siempre van directo. Default ON.
    #[serde(default = "default_true")]
    pub launch_via_steam: bool,
}

fn default_schema() -> u32 {
    CONFIG_SCHEMA
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: CONFIG_SCHEMA,
            install_root: None,
            subscribed_sets: Vec::new(),
            set_versions: HashMap::new(),
            publish_repo: None,
            publish_set_name: None,
            mod_sources: HashMap::new(),
            prefer_beta: false,
            mod_installed_tag: HashMap::new(),
            launch_via_steam: true,
        }
    }
}

/// Prefijo de una suscripcion por REPO (sigue el ULTIMO release) en `subscribed_sets`, para
/// distinguirla de una suscripcion por URL fija (clavada a un tag). Ej: `repo:YX14ng/sts2-mods`.
/// Una suscripcion por repo se resuelve a la URL del manifest del ultimo release en cada chequeo
/// (`transport::resolve_latest_manifest`), asi "actualizar" no obliga a re-pegar la URL.
pub const REPO_SUB_PREFIX: &str = "repo:";

/// Arma la entrada de `subscribed_sets` para una suscripcion por repo (`repo:owner/repo`).
pub fn repo_sub(owner_repo: &str) -> String {
    format!("{REPO_SUB_PREFIX}{owner_repo}")
}

/// Si `entry` es una suscripcion por repo (`repo:owner/repo`), devuelve `owner/repo`; si no, `None`.
pub fn as_repo_sub(entry: &str) -> Option<&str> {
    entry.strip_prefix(REPO_SUB_PREFIX)
}

/// Nombre legible para un set suscripto (la config guarda solo la URL/repo). Para una suscripcion
/// por repo (`repo:owner/repo`) devuelve "owner/repo (ultimo release)". Para una URL de GitHub
/// Release `.../USER/REPO/releases/download/TAG/...` devuelve "USER/REPO (TAG)"; si no matchea,
/// usa los ultimos dos segmentos de la ruta. Para mostrar en vez de la URL cruda.
pub fn set_label(url: &str) -> String {
    if let Some(repo) = as_repo_sub(url) {
        return format!("{repo} (ultimo release)");
    }
    let trimmed = url.split(['?', '#']).next().unwrap_or(url);
    let segs: Vec<&str> = trimmed
        .trim_end_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(pos) = segs.iter().position(|s| *s == "releases")
        && pos >= 2
        && segs.get(pos + 1) == Some(&"download")
        && let Some(tag) = segs.get(pos + 2)
    {
        return format!("{}/{} ({})", segs[pos - 2], segs[pos - 1], tag);
    }
    let n = segs.len();
    if n >= 2 {
        format!("{}/{}", segs[n - 2], segs[n - 1])
    } else {
        url.to_string()
    }
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("io", "Chaldea", "sts2-modsync")
}

pub fn config_path() -> Option<PathBuf> {
    Some(project_dirs()?.config_dir().join("config.toml"))
}

/// Directorio de datos de la app (`%APPDATA%/sts2-modsync/...`), junto al `config.toml`. Punto UNICO
/// de "donde viven los datos": el cache de hashes, el log, los perfiles y la clave de firma cuelgan
/// de aca (cada uno hace `data_dir()?.join("<archivo>")`).
pub fn data_dir() -> Option<PathBuf> {
    Some(config_path()?.parent()?.to_path_buf())
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
    save_to(&path, cfg)
}

/// Guarda `cfg` en `path` de forma ATOMICA: escribe a un `.tmp` y RENOMBRA sobre el destino (rename
/// es atomico en el mismo volumen, y en Windows reemplaza). `std::fs::write` TRUNCA primero: si la
/// app crashea/corta a mitad, dejaba un `config.toml` VACIO y se perdia TODA la config (install_root,
/// suscripciones, etc.). El parametro `path` es para poder testear sin tocar el %APPDATA% real.
fn save_to(path: &Path, cfg: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let s = toml::to_string_pretty(cfg)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, s).with_context(|| format!("escribiendo {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| {
        let _ = std::fs::remove_file(&tmp); // no dejar el .tmp colgado si el rename falla
        format!("reemplazando {}", path.display())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_to_es_atomico_y_round_trip() {
        let dir = std::env::temp_dir().join("sts2_modsync_cfg_save");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap(); // evitar la carrera delete-then-write en Windows
        let path = dir.join("config.toml");
        let cfg = Config {
            install_root: Some(std::path::PathBuf::from("/tmp/StS2")),
            subscribed_sets: vec!["repo:owner/set".into()],
            ..Config::default()
        };
        save_to(&path, &cfg).unwrap();
        // El destino existe, NO quedo ningun `.tmp` colgado, y re-leerlo conserva los campos.
        assert!(path.exists());
        assert!(!path.with_extension("toml.tmp").exists());
        let back = load_from(&path);
        assert_eq!(back.install_root, cfg.install_root);
        assert_eq!(back.subscribed_sets, cfg.subscribed_sets);
        // Sobrescribir (otro save) tambien anda y no deja `.tmp`.
        save_to(&path, &Config::default()).unwrap();
        assert!(!path.with_extension("toml.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        let mut set_versions = HashMap::new();
        set_versions.insert("https://a/b.json".to_string(), "1.2.3".to_string());
        let mut mod_sources = HashMap::new();
        mod_sources.insert("FGOCore".to_string(), "github:YX14ng/FGOCore".to_string());
        let cfg = Config {
            schema: CONFIG_SCHEMA,
            install_root: Some(PathBuf::from("/tmp/StS2")),
            subscribed_sets: vec!["https://a/b.json".into()],
            set_versions,
            publish_repo: Some("YX14ng/sts2-mods".into()),
            publish_set_name: Some("Mi Set".into()),
            mod_sources,
            prefer_beta: true,
            mod_installed_tag: HashMap::new(),
            launch_via_steam: false,
        };
        let back: Config = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(back.install_root, cfg.install_root);
        assert_eq!(back.subscribed_sets, cfg.subscribed_sets);
        assert_eq!(back.set_versions, cfg.set_versions);
        assert_eq!(back.publish_repo, cfg.publish_repo);
        assert_eq!(back.publish_set_name, cfg.publish_set_name);
        assert_eq!(back.mod_sources, cfg.mod_sources);
        assert_eq!(back.prefer_beta, cfg.prefer_beta);
        assert_eq!(back.launch_via_steam, cfg.launch_via_steam);
        assert_eq!(back.schema, CONFIG_SCHEMA);
    }

    #[test]
    fn launch_via_steam_default_true_en_config_vieja() {
        // Una config sin el campo (anterior a 1.13.0) debe quedar con launch_via_steam = true.
        let cfg: Config = toml::from_str("install_root = '/tmp/StS2'\n").unwrap();
        assert!(cfg.launch_via_steam);
    }

    #[test]
    fn repo_sub_round_trip_y_label() {
        let key = repo_sub("YX14ng/sts2-mods");
        assert_eq!(key, "repo:YX14ng/sts2-mods");
        assert_eq!(as_repo_sub(&key), Some("YX14ng/sts2-mods"));
        // una URL fija NO es una suscripcion por repo.
        assert_eq!(
            as_repo_sub("https://github.com/a/b/releases/download/v1/x.json"),
            None
        );
        // label legible de la suscripcion por repo.
        assert_eq!(set_label(&key), "YX14ng/sts2-mods (ultimo release)");
    }

    #[test]
    fn set_label_github_y_fallback() {
        assert_eq!(
            set_label(
                "https://github.com/YX14ng/sts2-mods/releases/download/2026.06.14/set-manifest.json"
            ),
            "YX14ng/sts2-mods (2026.06.14)"
        );
        // no-GitHub: ultimos dos segmentos.
        assert_eq!(
            set_label("https://example.com/miset/set-manifest.json"),
            "miset/set-manifest.json"
        );
    }
}
