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
