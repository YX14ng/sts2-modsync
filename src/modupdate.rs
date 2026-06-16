//! Auto-update de MODS instalados desde su upstream (`modsource::ModSource`). Fase 1: GitHub
//! (releases). Por cada mod con origen conocido, chequea la ultima version segun el canal GLOBAL
//! (estable = MAIN, beta = pre-releases) y, si hay una mas nueva, baja el asset `.zip` e instala
//! reemplazando (preservando si estaba habilitado o no). Nexus llega en la fase 2 (handler `nxm://`).
//!
//! SEGURIDAD: el `.zip` se baja por HTTPS (con un tope de tamaño y redirect-policy https-only) y se
//! instala SOLO si el `<id>.json` dentro del zip declara el mismo id que se actualiza
//! (`manager::install_update_zip`), asi un release del upstream de A no puede pisar a B. No hay firma
//! por-mod (el ancla es el repo upstream que el usuario eligio como origen).

use crate::detect::Install;
use crate::modsource::ModSource;
use crate::{config, manager, modlist, transport, update};
use anyhow::{Context, Result, bail};

/// Techo duro de la descarga de un asset: un `.zip` mas grande se rechaza (no llenar el disco con un
/// release-bomba). Holgado para mods con `.pck` grandes, pero acotado.
const DOWNLOAD_MAX: u64 = 2 * 1024 * 1024 * 1024;

/// Coordenadas para bajar DIRECTO de Nexus (solo Premium): el archivo MAIN ya resuelto. Con esto,
/// `apply_nexus` resuelve el download-link en el momento (es de vida corta) y baja+instala sin el
/// flujo `nxm://`. `None` para GitHub o para Nexus gratis (que sigue por `nxm://`).
#[derive(Debug, Clone)]
pub struct NexusRef {
    pub game: String,
    pub mod_id: u64,
    pub file_id: u64,
}

/// Una actualizacion disponible para un mod.
#[derive(Debug, Clone)]
pub struct ModUpdate {
    pub mod_id: String,
    /// Version instalada (del `<id>.json`); `None` si el mod no declara version.
    pub current: Option<String>,
    /// Version disponible (derivada del tag del release).
    pub latest: String,
    pub tag: String,
    /// `true` si la version disponible es un pre-release (canal BETA).
    pub prerelease: bool,
    /// URL del `.zip` a bajar (asset del release, o el source zipball como fallback). VACIO en Nexus.
    pub asset_url: String,
    /// Pagina del release (para "Abrir").
    pub html_url: String,
    /// Solo Nexus Premium: el archivo a bajar directo (`apply_nexus`). `None` = GitHub o Nexus gratis.
    pub nexus: Option<NexusRef>,
}

/// Chequea si hay una version mas nueva de `mod_id` en GitHub. `current` = version del `<id>.json`
/// (puede ser `None`); `installed_tag` = el ultimo tag que ESTE programa instalo para el mod (de
/// `config.mod_installed_tag`), para no re-ofrecer la misma version a un mod sin `version`.
/// `prefer_beta` = canal global. `Ok(None)` = ya estas al dia o no hay release en ese canal. Usa el
/// token de GitHub guardado si lo hay (sube el limite anon de 60/h a 5000/h).
pub fn check_github(
    owner: &str,
    repo: &str,
    mod_id: &str,
    current: Option<&str>,
    installed_tag: Option<&str>,
    prefer_beta: bool,
) -> Result<Option<ModUpdate>> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases?per_page=100");
    let mut req = transport::http_client()?
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(std::time::Duration::from_secs(45)); // respuesta chica: timeout total seguro
    if let Some(tok) = crate::github::load_token() {
        req = req.bearer_auth(tok); // 5000/h autenticado vs 60/h anonimo
    }
    let resp = req.send().with_context(|| format!("GET {url}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("el repo {owner}/{repo} no existe o no tiene releases");
    }
    // Token invalido / rate-limit -> mensaje claro (mismo decode compartido que los GET de transport).
    let body = crate::github::check_api_status(resp)?
        .error_for_status()
        .context("github api (releases)")?
        .text()?;
    let arr: Vec<serde_json::Value> =
        serde_json::from_str(&body).context("json invalido de releases")?;
    let Some(pick) = pick_release(&arr, mod_id, prefer_beta) else {
        return Ok(None); // no hay release en este canal
    };
    // ¿Ya instalamos EXACTAMENTE este release antes? (cubre los mods sin `version` en su `<id>.json`,
    // que sino se ofrecerian en loop). Igualdad de tag exacta, robusta ante tags no-semver.
    if installed_tag == Some(pick.tag.as_str()) {
        return Ok(None);
    }
    let is_new = match current {
        Some(cur) => update::is_newer(&pick.version, cur),
        None => true, // sin version declarada: ofrecer (salvo que ya sea este tag, cubierto arriba)
    };
    if !is_new {
        return Ok(None);
    }
    Ok(Some(ModUpdate {
        mod_id: mod_id.to_string(),
        current: current.map(str::to_string),
        latest: pick.version,
        tag: pick.tag,
        prerelease: pick.prerelease,
        asset_url: pick.asset_url,
        html_url: pick.html_url,
        nexus: None,
    }))
}

/// Un release elegido (campos planos, testeable sin red).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Picked {
    tag: String,
    version: String,
    prerelease: bool,
    asset_url: String,
    html_url: String,
}

/// Tag de un release JSON (`""` si falta).
fn release_tag(r: &serde_json::Value) -> &str {
    r.get("tag_name").and_then(|x| x.as_str()).unwrap_or("")
}

/// Elige el release a seguir de la lista `/releases`. Ignora drafts y, en canal estable, los
/// pre-releases. Entre los elegibles toma el de MAYOR version (`parse_ver` del tag); en empate, el
/// primero del array (GitHub lo devuelve del mas nuevo al mas viejo por fecha). Del release elegido
/// saca el asset `.zip` cuyo nombre contiene `mod_id` (para desambiguar releases con varios zips);
/// si no, el primer `.zip`; si no hay, el source zipball. `None` si no hay candidato.
fn pick_release(arr: &[serde_json::Value], mod_id: &str, prefer_beta: bool) -> Option<Picked> {
    let chosen = arr
        .iter()
        .filter(|r| {
            let draft = r
                .get("draft")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let pre = r
                .get("prerelease")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            !draft && (prefer_beta || !pre) && !release_tag(r).is_empty()
        })
        // mayor version; empate -> el `best` (mas nuevo en el array, por venir antes).
        .reduce(|best, r| {
            if update::parse_ver(release_tag(r)) > update::parse_ver(release_tag(best)) {
                r
            } else {
                best
            }
        })?;
    let tag = release_tag(chosen);
    let prerelease = chosen
        .get("prerelease")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let html_url = chosen
        .get("html_url")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let assets = chosen.get("assets").and_then(|a| a.as_array());
    let id_lower = mod_id.to_ascii_lowercase();
    // Preferir el `.zip` que mencione el mod; si no, cualquier `.zip`; si no, el source zipball.
    let zip = |want_id: bool| -> Option<String> {
        assets?.iter().find_map(|a| {
            let name = a.get("name")?.as_str()?.to_ascii_lowercase();
            if name.ends_with(".zip") && (!want_id || name.contains(&id_lower)) {
                a.get("browser_download_url")?.as_str().map(str::to_string)
            } else {
                None
            }
        })
    };
    let asset_url = zip(true).or_else(|| zip(false)).or_else(|| {
        chosen
            .get("zipball_url")
            .and_then(|x| x.as_str())
            .map(str::to_string)
    })?;
    Some(Picked {
        tag: tag.to_string(),
        version: tag.trim_start_matches('v').to_string(),
        prerelease,
        asset_url,
        html_url,
    })
}

/// Aplica una actualizacion: baja el `.zip` de `asset_url` e instala reemplazando el mod `mod_id`
/// (solo si el zip declara ese MISMO id), preservando si estaba habilitado o no, y recuerda `tag` en
/// `config.mod_installed_tag`. Exige el juego cerrado (lo verifica `manager`).
pub fn apply(install: &Install, mod_id: &str, asset_url: &str, tag: &str) -> Result<()> {
    transport::require_https(asset_url)?;
    let was_disabled = manager::mod_dir(install, mod_id)
        .is_some_and(|d| d.starts_with(modlist::disabled_dir(install)));

    let tmp = std::env::temp_dir().join(format!(
        "sts2_modupdate_{}.zip",
        crate::util::unique_nanos()
    ));
    // Bajar + instalar; limpiar el `.zip` temporal SIEMPRE (exito o error).
    let res = (|| {
        transport::download_capped(asset_url, &tmp, DOWNLOAD_MAX)?;
        manager::install_update_zip(install, &tmp, mod_id)
    })();
    let _ = std::fs::remove_file(&tmp);
    res?;

    // El id quedo garantizado == mod_id (lo valido install_update_zip). Restaurar deshabilitado.
    if was_disabled {
        manager::disable(install, mod_id)
            .with_context(|| format!("re-deshabilitando {mod_id} tras actualizar"))?;
    }
    // Recordar el tag instalado (para no re-ofrecer la misma version a un mod sin `version`).
    let mut cfg = config::load();
    cfg.mod_installed_tag
        .insert(mod_id.to_string(), tag.to_string());
    let _ = config::save(&cfg);
    Ok(())
}

/// Chequeo de un mod cuyo origen es NEXUS. Devuelve la version disponible; si el usuario es
/// **Premium** (`premium`), ademas resuelve el archivo MAIN para poder bajarlo DIRECTO (`nexus:
/// Some(..)` -> el GUI muestra "Actualizar"). Si NO es Premium, `nexus` queda `None` y la descarga
/// va por el handler `nxm://`. `installed_tag` evita re-ofrecer una version ya instalada cuyo
/// `<id>.json` no refleje el bump. `nexus_mod_id` es el id del mod EN Nexus.
pub fn check_nexus(
    mod_id: &str,
    game: &str,
    nexus_mod_id: u64,
    current: Option<&str>,
    installed_tag: Option<&str>,
    premium: bool,
) -> Result<Option<ModUpdate>> {
    let Some(nx) = crate::nexus::check(game, nexus_mod_id, current)? else {
        return Ok(None);
    };
    // Ya instalamos exactamente esta version antes (cubre mods cuyo <id>.json no refleja el bump).
    if installed_tag == Some(nx.latest.as_str()) {
        return Ok(None);
    }
    // Premium: resolver el archivo MAIN para bajar directo. Free: queda None (flujo nxm://). Si la
    // resolucion falla, no abortamos el chequeo: se cae al flujo nxm:// igual.
    let nexus = if premium {
        crate::nexus::latest_main_file(game, nexus_mod_id)
            .ok()
            .flatten()
            .map(|f| NexusRef {
                game: game.to_string(),
                mod_id: nexus_mod_id,
                file_id: f.file_id,
            })
    } else {
        None
    };
    Ok(Some(ModUpdate {
        mod_id: mod_id.to_string(),
        current: current.map(str::to_string),
        latest: nx.latest.clone(),
        tag: nx.latest,
        prerelease: false,
        asset_url: String::new(), // Nexus: la descarga directa va por `nexus`/`apply_nexus`
        html_url: format!("https://www.nexusmods.com/{game}/mods/{nexus_mod_id}"),
        nexus,
    }))
}

/// Aplica una actualizacion DIRECTA de Nexus (solo Premium): resuelve el download-link del archivo
/// MAIN (de vida corta, por eso se resuelve recien aca), baja el archivo e instala REEMPLAZANDO
/// `mod_id` (solo si el archivo declara ese MISMO id, via `manager::install_update_zip`), preservando
/// enable/disable y recordando la version. Soporta `.zip` y `.7z`; `.rar`/otros no se auto-instalan:
/// se avisa para bajarlos a mano. Exige el juego cerrado (lo verifica `manager`).
pub fn apply_nexus(install: &Install, mod_id: &str, nref: &NexusRef, version: &str) -> Result<()> {
    // Premium: download-link directo (sin key/expires de un solo uso).
    let url = crate::nexus::download_link(&nref.game, nref.mod_id, nref.file_id, None, None)?;
    if !url_looks_archive(&url) {
        bail!(
            "el archivo de Nexus no es .zip ni .7z y no se puede auto-instalar. Bajalo desde la \
             pagina del mod y usa 'Instalar .zip' (pestaña Mods)."
        );
    }
    // HTTPS lo exige `download_capped` (require_https + cliente https-only); no hace falta repetirlo.
    let was_disabled = manager::mod_dir(install, mod_id)
        .is_some_and(|d| d.starts_with(modlist::disabled_dir(install)));

    let tmp =
        std::env::temp_dir().join(format!("sts2_nexusupd_{}.bin", crate::util::unique_nanos()));
    let res = (|| {
        transport::download_capped(&url, &tmp, DOWNLOAD_MAX)?;
        manager::install_update_zip(install, &tmp, mod_id)
    })();
    let _ = std::fs::remove_file(&tmp);
    res?;

    if was_disabled {
        manager::disable(install, mod_id)
            .with_context(|| format!("re-deshabilitando {mod_id} tras actualizar"))?;
    }
    let mut cfg = config::load();
    cfg.mod_installed_tag
        .insert(mod_id.to_string(), version.to_string());
    let _ = config::save(&cfg);
    Ok(())
}

/// `true` si el ultimo segmento del path de `url` (sin query/fragment) termina en `.zip` o `.7z` (los
/// formatos que se auto-instalan). Pre-filtro para no bajar un `.rar` que igual no se podria instalar;
/// el formato REAL lo decide `manager::archive_kind` por MAGIC tras bajar.
fn url_looks_archive(url: &str) -> bool {
    let ext = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase());
    matches!(ext.as_deref(), Some("zip") | Some("7z"))
}

/// Contexto del chequeo de update: canal global + estado de las cuentas. Junta los flags que antes
/// se pasaban sueltos en cada call-site (es `Copy`, se reusa en el loop de "chequear todos").
#[derive(Debug, Clone, Copy, Default)]
pub struct CheckCtx {
    /// Canal global: `true` = beta (pre-releases), `false` = estable (MAIN).
    pub prefer_beta: bool,
    /// Hay una API key de Nexus conectada (si no, los mods de Nexus no se pueden chequear).
    pub nexus_connected: bool,
    /// La cuenta de Nexus es Premium (habilita la descarga directa; si no, va por `nxm://`).
    pub nexus_premium: bool,
}

/// Chequea si hay una version nueva de un mod segun su `ModSource`, despachando a `check_github` /
/// `check_nexus`. Para Nexus, si NO hay API key conectada devuelve `Ok(None)` (no se puede chequear,
/// pero NO es un error: el caller lo trata como "sin novedad"). `current` = version del `<id>.json`;
/// `installed_tag` = el ultimo tag que instalamos. UNICO punto de dispatch (antes estaba duplicado en
/// los dos workers del GUI, que ademas diferian en el guard de Nexus).
pub fn check(
    src: &ModSource,
    mod_id: &str,
    current: Option<&str>,
    installed_tag: Option<&str>,
    ctx: CheckCtx,
) -> Result<Option<ModUpdate>> {
    match src {
        ModSource::GitHub { owner, repo } => {
            check_github(owner, repo, mod_id, current, installed_tag, ctx.prefer_beta)
        }
        ModSource::Nexus {
            game,
            mod_id: nexus_mod_id,
        } => {
            if !ctx.nexus_connected {
                return Ok(None); // sin API key no se puede chequear Nexus (no es un fallo)
            }
            check_nexus(
                mod_id,
                game,
                *nexus_mod_id,
                current,
                installed_tag,
                ctx.nexus_premium,
            )
        }
    }
}

/// El origen efectivo de un mod: el override del usuario en `config.mod_sources` (prioridad) o el
/// hint declarado en el `<id>.json`. `None` si no hay ninguno.
pub fn effective_source(
    m: &modlist::InstalledMod,
    cfg: &crate::config::Config,
) -> Option<ModSource> {
    cfg.mod_sources
        .get(m.id())
        .and_then(|s| ModSource::parse(s))
        .or_else(|| m.manifest.source_hint())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(tag: &str, prerelease: bool, assets: &[&str]) -> serde_json::Value {
        let assets: Vec<_> = assets
            .iter()
            .map(|name| {
                json!({ "name": name, "browser_download_url": format!("https://github.com/o/r/releases/download/{tag}/{name}") })
            })
            .collect();
        json!({
            "tag_name": tag,
            "prerelease": prerelease,
            "draft": false,
            "html_url": format!("https://github.com/o/r/releases/tag/{tag}"),
            "zipball_url": format!("https://api.github.com/repos/o/r/zipball/{tag}"),
            "assets": assets,
        })
    }

    #[test]
    fn pick_release_respeta_el_canal() {
        let arr = vec![
            release("v2.0.0-beta", true, &["Mod.zip"]),
            release("v1.2.0", false, &["Mod.zip"]),
            release("v1.1.0", false, &["Mod.zip"]),
        ];
        let stable = pick_release(&arr, "Mod", false).unwrap();
        assert_eq!(stable.tag, "v1.2.0");
        assert!(!stable.prerelease);
        let beta = pick_release(&arr, "Mod", true).unwrap();
        assert_eq!(beta.tag, "v2.0.0-beta");
        assert!(beta.prerelease);
    }

    #[test]
    fn check_nexus_desconectado_da_none_sin_red() {
        // Sin API key de Nexus conectada, `check` devuelve Ok(None) sin tocar la red (no es un fallo).
        let src = ModSource::Nexus {
            game: "slaythespire2".into(),
            mod_id: 42,
        };
        let r = check(
            &src,
            "Mod",
            Some("1.0"),
            None,
            CheckCtx {
                nexus_connected: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn url_looks_archive_acepta_zip_y_7z() {
        assert!(url_looks_archive(
            "https://cdn.nexus/files/Mod-1.2.zip?md5=x&expires=9"
        ));
        assert!(url_looks_archive("https://cdn/a/b/Mod.ZIP")); // case-insensitive
        assert!(url_looks_archive("https://cdn/files/Mod-1.2.7z?md5=x")); // .7z ahora si
        assert!(!url_looks_archive("https://cdn/files/Mod.rar")); // .rar no
        assert!(!url_looks_archive("https://cdn/zip/Mod")); // 'zip' en el path, no en la extension
    }

    #[test]
    fn pick_release_elige_por_version_no_por_orden_del_array() {
        // Hotfix viejo publicado DESPUES (primero en el array) no debe ganarle a la version mayor.
        let arr = vec![
            release("v1.4.1", false, &["Mod.zip"]),
            release("v2.0.0", false, &["Mod.zip"]),
        ];
        assert_eq!(pick_release(&arr, "Mod", false).unwrap().tag, "v2.0.0");
    }

    #[test]
    fn pick_release_prefiere_el_zip_del_mod_y_cae_al_zipball() {
        // Varios zips: gana el que menciona el mod_id.
        let arr = vec![release("v1", false, &["Source.zip", "FGOCore.zip"])];
        let p = pick_release(&arr, "FGOCore", false).unwrap();
        assert!(p.asset_url.ends_with("FGOCore.zip"));
        // Sin asset .zip -> source zipball.
        let arr2 = vec![release("v1", false, &["Mod.dll"])];
        assert!(
            pick_release(&arr2, "Mod", false)
                .unwrap()
                .asset_url
                .contains("zipball")
        );
    }

    #[test]
    fn pick_release_ignora_draft_y_solo_beta_en_estable_da_none() {
        let mut draft = release("v3.0.0", false, &["Mod.zip"]);
        draft["draft"] = json!(true);
        let arr = vec![draft, release("v1.0.0", false, &["Mod.zip"])];
        assert_eq!(pick_release(&arr, "Mod", false).unwrap().tag, "v1.0.0");

        let only_beta = vec![release("v1.0.0-rc1", true, &["Mod.zip"])];
        assert!(pick_release(&only_beta, "Mod", false).is_none()); // estable: nada
        assert!(pick_release(&only_beta, "Mod", true).is_some()); // beta: si
    }
}
