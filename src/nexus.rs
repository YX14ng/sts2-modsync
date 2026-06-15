//! API de Nexus Mods (v1 REST). Fase 2a: validar la API Key personal del usuario y CHEQUEAR la
//! version disponible de un mod. La DESCARGA automatica (`download_link` + handler `nxm://`) es la
//! fase 2b: para usuarios gratis Nexus exige el flujo `nxm` (un `key`/`expires` que genera la web al
//! tocar "Mod Manager Download"); con Premium la API devuelve el link directo.
//!
//! La API Key personal se saca de la cuenta de Nexus (Preferences -> API). Identifica al usuario; se
//! guarda SEGURO en el llavero del SO (como el token de GitHub), nunca en texto plano. El cliente que
//! solo usa GitHub NO necesita nada de esto.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const API: &str = "https://api.nexusmods.com/v1";
const KEYRING_SERVICE: &str = "sts2-modsync";
const KEYRING_USER: &str = "nexus-apikey";

// --- API key (guardada en el llavero del SO) --------------------------------

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).context("abriendo el llavero del SO")
}

/// Guarda la API Key personal de Nexus en el llavero.
pub fn store_key(key: &str) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        bail!("la API key esta vacia");
    }
    entry()?
        .set_password(key)
        .context("guardando la API key de Nexus en el llavero")
}

/// Lee la API Key guardada, si hay.
pub fn load_key() -> Option<String> {
    entry().ok()?.get_password().ok()
}

/// Borra la API Key guardada (desconectar). No falla si no habia.
pub fn clear_key() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow::Error::new(e).context("borrando la API key del llavero")),
    }
}

pub fn is_connected() -> bool {
    load_key().is_some()
}

/// Cliente para Nexus SIN seguir redirects: la API key va en un header CUSTOM (`apikey`) que reqwest
/// NO strippea en un redirect cross-host (solo strippea Authorization/Cookie). Deshabilitar redirects
/// evita filtrar la key si Nexus respondiera un 30x hacia otro host. Los endpoints v1 devuelven JSON
/// directo (no redirigen), asi que no se pierde nada.
fn nexus_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("sts2-modsync/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("construir cliente http de Nexus")
}

fn get(url: &str, key: &str) -> Result<reqwest::blocking::Response> {
    nexus_client()?
        .get(url)
        .header("apikey", key)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .with_context(|| format!("GET {url}"))
}

// --- endpoints --------------------------------------------------------------

/// Usuario autenticado (de `validate.json`): para mostrar "conectado como X" + si es Premium.
#[derive(Debug, Clone, Deserialize)]
pub struct NexusUser {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub is_premium: bool,
}

/// Valida la API Key guardada contra Nexus. Devuelve el usuario o error claro (sin key / invalida).
pub fn validate() -> Result<NexusUser> {
    let key = load_key().context("no hay API key de Nexus guardada")?;
    let resp = get(&format!("{API}/users/validate.json"), &key)?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        bail!("la API key de Nexus es invalida (revisa Preferences -> API en tu cuenta)");
    }
    resp.error_for_status()
        .context("nexus api (validate)")?
        .json::<NexusUser>()
        .context("parseando la respuesta de validate")
}

/// Mod info (`/games/{game}/mods/{id}.json`): la `version` es la headline del mod (su archivo MAIN).
#[derive(Debug, Deserialize)]
struct ModInfo {
    #[serde(default)]
    version: String,
    #[serde(default)]
    name: String,
}

/// Una version disponible de un mod en Nexus (la descarga es fase 2b: abrir la pagina).
#[derive(Debug, Clone)]
pub struct NexusUpdate {
    pub game: String,
    pub mod_id: u64,
    /// Nombre del mod en Nexus.
    pub name: String,
    /// Version disponible (headline del mod).
    pub latest: String,
}

/// Chequea la version disponible de un mod en Nexus. `current` = version instalada. `Ok(None)` si ya
/// estas al dia o el mod no expone version. Necesita la API key guardada. Nota: Nexus NO tiene un
/// canal "beta" formal, asi que el toggle estable/beta no aplica aca (se usa la headline del mod).
pub fn check(game: &str, mod_id: u64, current: Option<&str>) -> Result<Option<NexusUpdate>> {
    let key = load_key().context("conecta tu API key de Nexus para chequear versiones")?;
    let url = format!("{API}/games/{game}/mods/{mod_id}.json");
    let resp = get(&url, &key)?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        bail!("la API key de Nexus es invalida");
    }
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("el mod {game}/{mod_id} no existe en Nexus (¿game domain o id equivocado?)");
    }
    let info: ModInfo = resp
        .error_for_status()
        .context("nexus api (mod info)")?
        .json()
        .context("parseando el mod info de Nexus")?;
    let latest = info.version.trim();
    if latest.is_empty() {
        return Ok(None);
    }
    let is_new = match current {
        Some(cur) => version_differs(latest, cur),
        None => true,
    };
    if !is_new {
        return Ok(None);
    }
    Ok(Some(NexusUpdate {
        game: game.to_string(),
        mod_id,
        name: info.name,
        latest: latest.to_string(),
    }))
}

/// Una URL de descarga del CDN (de `download_link.json`).
#[derive(Debug, Deserialize)]
struct DownloadLink {
    #[serde(rename = "URI")]
    uri: String,
}

/// Resuelve la URL de descarga (CDN) de un archivo de un mod (fase 2b). `key`/`expires` vienen del
/// link `nxm://` (un solo uso, los genera la web para usuarios gratis); para Premium pueden ir vacios
/// y la API devuelve el link directo. Devuelve la PRIMERA URI. Necesita la API key guardada.
pub fn download_link(
    game: &str,
    mod_id: u64,
    file_id: u64,
    key: Option<&str>,
    expires: Option<&str>,
) -> Result<String> {
    let apikey = load_key().context("conecta tu API key de Nexus (nexus-login) para descargar")?;
    let url = format!("{API}/games/{game}/mods/{mod_id}/files/{file_id}/download_link.json");
    // `key`/`expires` (de un solo uso) van por `.query()` para que reqwest los url-encodee. OJO: reqwest
    // adjunta la URL COMPLETA (con `?key=..&expires=..`) a su PROPIO error, y `{e:#}` la imprimiria en
    // el dialogo/stderr — por eso a cada error de reqwest le sacamos la URL con `without_url()` ANTES de
    // envolverlo, y el contexto propio usa la URL BASE (sin la query). Asi la credencial no se filtra.
    let mut req = nexus_client()?
        .get(&url)
        .header("apikey", &apikey)
        .header(reqwest::header::ACCEPT, "application/json");
    if let (Some(k), Some(e)) = (key, expires) {
        req = req.query(&[("key", k), ("expires", e)]);
    }
    let resp = req
        .send()
        .map_err(|e| e.without_url())
        .with_context(|| format!("GET {url}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        bail!("la API key de Nexus es invalida");
    }
    if resp.status() == reqwest::StatusCode::FORBIDDEN {
        bail!(
            "Nexus rechazo la descarga (403): para bajar gratis hay que iniciar desde \"Mod Manager \
             Download\" en la pagina del mod (genera un link de un solo uso), o ser Premium"
        );
    }
    let body = resp
        .error_for_status()
        .map_err(|e| e.without_url())
        .context("nexus api (download_link)")?
        .text()
        .map_err(|e| e.without_url())
        .context("leyendo el download_link de Nexus")?;
    let links: Vec<DownloadLink> =
        serde_json::from_str(&body).context("parseando el download_link de Nexus")?;
    links
        .into_iter()
        .map(|l| l.uri)
        .find(|u| !u.is_empty())
        .context("Nexus no devolvio una URL de descarga")
}

/// Un archivo de un mod en Nexus (de `files.json`), para resolver cual bajar en Premium.
#[derive(Debug, Clone, Deserialize)]
pub struct NexusFile {
    pub file_id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// Categoria del archivo (1=MAIN, 2=UPDATE, 3=OPTIONAL, 4=OLD_VERSION, 6=DELETED, 7=ARCHIVED).
    #[serde(default)]
    category_id: Option<u64>,
    #[serde(default)]
    is_primary: bool,
}

#[derive(Deserialize)]
struct FilesResp {
    #[serde(default)]
    files: Vec<NexusFile>,
}

/// El archivo MAIN/primario de un mod, para bajarlo directo (Premium). Prefiere el marcado
/// `is_primary`, luego la categoria MAIN (id 1), luego el de mayor `file_id`; ignora versiones
/// viejas/borradas/archivadas. `Ok(None)` si el mod no tiene un archivo instalable. Necesita la API
/// key guardada.
pub fn latest_main_file(game: &str, mod_id: u64) -> Result<Option<NexusFile>> {
    let key = load_key().context("conecta tu API key de Nexus para resolver el archivo a bajar")?;
    let url = format!("{API}/games/{game}/mods/{mod_id}/files.json");
    let resp = get(&url, &key)?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        bail!("la API key de Nexus es invalida");
    }
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("el mod {game}/{mod_id} no existe en Nexus");
    }
    let parsed: FilesResp = resp
        .error_for_status()
        .context("nexus api (files)")?
        .json()
        .context("parseando los archivos del mod de Nexus")?;
    Ok(pick_main_file(&parsed.files).cloned())
}

/// Elige el archivo a bajar de la lista de un mod: `is_primary`, luego categoria MAIN (1), luego el
/// de mayor `file_id`; ignora versiones viejas/borradas/archivadas (categorias 4/6/7). Helper PURO
/// (testeable sin red) de [`latest_main_file`].
fn pick_main_file(files: &[NexusFile]) -> Option<&NexusFile> {
    // No instalables: OLD_VERSION (4), DELETED (6), ARCHIVED (7).
    let usable = |f: &&NexusFile| !matches!(f.category_id, Some(4) | Some(6) | Some(7));
    files
        .iter()
        .filter(usable)
        .find(|f| f.is_primary)
        .or_else(|| {
            files
                .iter()
                .filter(usable)
                .find(|f| f.category_id == Some(1))
        })
        .or_else(|| files.iter().filter(usable).max_by_key(|f| f.file_id))
}

/// "Hay una version distinta disponible" para versiones de Nexus, que son TEXTO LIBRE (ej "1.2",
/// "v2", "Beta 3", "2024-05-01"). Si AMBAS son semver puro (`vX.Y.Z`), usa la comparacion numerica
/// (`is_newer`, confiable y sin falsos positivos por "1.0" vs "1.0.0"). Si alguna NO lo es (fecha,
/// nombre, sufijo), cae a comparar los strings: distinto = posible update — `parse_ver` mangearia
/// las fechas ("2024-05-01" -> (2024,0,0)) y PERDERIA updates.
fn version_differs(latest: &str, current: &str) -> bool {
    if is_pure_semver(latest) && is_pure_semver(current) {
        crate::update::is_newer(latest, current)
    } else {
        !latest.trim().eq_ignore_ascii_case(current.trim())
    }
}

/// `true` si `s` es un `X.Y.Z...` solo de digitos y puntos (con `v` opcional): ahi `parse_ver` es
/// confiable. Una fecha ("2024-05-01"), un nombre ("Beta 3") o un sufijo ("1.2a") NO lo son.
fn is_pure_semver(s: &str) -> bool {
    let s = s.trim().trim_start_matches('v');
    !s.is_empty()
        && s.split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(file_id: u64, category_id: Option<u64>, is_primary: bool) -> NexusFile {
        NexusFile {
            file_id,
            name: String::new(),
            version: String::new(),
            category_id,
            is_primary,
        }
    }

    #[test]
    fn pick_main_file_prioriza_primary_luego_main_luego_mayor_id() {
        // is_primary gana aunque no sea el de mayor id ni categoria MAIN.
        let fs = vec![
            file(10, Some(1), false),
            file(20, Some(3), true), // OPTIONAL pero primary
        ];
        assert_eq!(pick_main_file(&fs).unwrap().file_id, 20);
        // sin primary: gana la categoria MAIN (1).
        let fs = vec![file(30, Some(3), false), file(5, Some(1), false)];
        assert_eq!(pick_main_file(&fs).unwrap().file_id, 5);
        // sin primary ni MAIN: el de mayor file_id entre los instalables.
        let fs = vec![file(7, Some(2), false), file(9, Some(5), false)];
        assert_eq!(pick_main_file(&fs).unwrap().file_id, 9);
        // ignora viejas/borradas/archivadas (4/6/7) aunque tengan id mayor.
        let fs = vec![file(100, Some(4), false), file(8, Some(2), false)];
        assert_eq!(pick_main_file(&fs).unwrap().file_id, 8);
        // todo descartado / vacio -> None.
        assert!(pick_main_file(&[]).is_none());
        assert!(pick_main_file(&[file(1, Some(6), false)]).is_none());
    }

    #[test]
    fn version_differs_semver_y_texto_libre() {
        // semver: mayor -> nuevo; igual/menor -> no.
        assert!(version_differs("1.2.0", "1.1.0"));
        assert!(!version_differs("1.0.0", "1.0.0"));
        assert!(!version_differs("1.0", "1.0.0")); // distinto string pero MISMO semver -> no
        // texto libre (ambas no-parseables): distinto -> nuevo; igual -> no.
        assert!(version_differs("Beta 4", "Beta 3"));
        assert!(!version_differs("Beta 3", "beta 3")); // case-insensitive
        assert!(version_differs("2024-05-01", "2024-04-01"));
    }
}
