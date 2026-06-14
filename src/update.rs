//! Auto-update: chequea el ultimo GitHub Release del propio repo y, si hay una version
//! nueva, baja el zip, extrae el exe, se reemplaza (`self-replace` maneja el exe en uso en
//! Windows) y relanza. Es **best-effort**: si no hay red ni releases, no molesta.
//!
//! SEGURIDAD: baja y EJECUTA un binario del PROPIO release por **HTTPS**; el ancla de
//! confianza es el dueño del repo (vos). Estandar para auto-update; sin firma extra.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::path::Path;

const OWNER: &str = "YX14ng";
const REPO: &str = "sts2-modsync";
/// Exe (dentro del zip del release) que se extrae y reemplaza al actualizar el GUI.
const ASSET_EXE: &str = "sts2-modsync-gui.exe";
const UA: &str = concat!("sts2-modsync/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub notes: String,
    pub html_url: String,
    pub zip_url: String,
}

/// Version actual del binario (de Cargo.toml).
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(UA)
        .build()
        .context("construir cliente http")
}

/// Consulta el ultimo release. `Ok(None)` si todavia no hay releases (404). Error solo si
/// la red o el parseo fallan.
pub fn check_latest() -> Result<Option<Release>> {
    // Listamos los releases (NO `/latest`) y elegimos el de mayor tag `vX.Y.Z`. Asi los releases
    // de SETS DE MODS (tags tipo `2026.06.14`) que el usuario publique en el MISMO repo NO se
    // confunden con una version nueva de la app.
    let url = format!("https://api.github.com/repos/{OWNER}/{REPO}/releases?per_page=30");
    let resp = client()?
        .get(&url)
        .send()
        .with_context(|| format!("GET {url}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let body = resp.error_for_status().context("github api")?.text()?;
    let arr: Vec<serde_json::Value> = serde_json::from_str(&body).context("json invalido")?;
    Ok(pick_best_release(&arr).and_then(release_from_json))
}

/// Elige el release de MAYOR version con tag `vX.Y.Z` (ignora drafts y tags que NO empiezan
/// con `v`, p.ej. releases de SETS DE MODS tipo `2026.06.14` publicados en el mismo repo).
/// Helper testeable de `check_latest`.
fn pick_best_release(arr: &[serde_json::Value]) -> Option<&serde_json::Value> {
    arr.iter()
        .filter(|r| {
            !r.get("draft")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .filter(|r| {
            r.get("tag_name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|t| t.starts_with('v'))
        })
        .max_by_key(|r| {
            parse_ver(
                r.get("tag_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(""),
            )
        })
}

/// Construye un `Release` a partir del JSON de un release de la API de GitHub.
fn release_from_json(v: &serde_json::Value) -> Option<Release> {
    let tag = v.get("tag_name").and_then(|x| x.as_str())?.to_string();
    if tag.is_empty() {
        return None;
    }
    let str_field = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    // primer asset cuyo nombre termina en .zip
    let zip_url = v
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|asset| {
                let name = asset.get("name")?.as_str()?;
                if name.to_ascii_lowercase().ends_with(".zip") {
                    asset
                        .get("browser_download_url")?
                        .as_str()
                        .map(str::to_string)
                } else {
                    None
                }
            })
        })
        .unwrap_or_default();

    Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        tag,
        notes: str_field("body"),
        html_url: str_field("html_url"),
        zip_url,
    })
}

/// `Some(release)` si hay una version MAYOR a la actual; `None` si estas al dia o si algo
/// fallo (best-effort: el auto-check no debe romper la app).
pub fn check() -> Option<Release> {
    let rel = check_latest().ok()??;
    is_newer(&rel.version, current_version()).then_some(rel)
}

/// Compara dos versiones "X.Y.Z" (con o sin 'v'; ignora sufijos `-pre`/`+build`). True si
/// `latest` es mayor que `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    parse_ver(latest) > parse_ver(current)
}

fn parse_ver(s: &str) -> (u64, u64, u64) {
    let s = s.trim().trim_start_matches('v');
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let mut it = core.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// Baja el zip del release, extrae `ASSET_EXE`, reemplaza el exe en uso, relanza y sale.
/// En exito NO retorna (hace `exit(0)` tras relanzar la version nueva).
pub fn apply(rel: &Release) -> Result<()> {
    if rel.zip_url.is_empty() {
        bail!("el release {} no trae un asset .zip", rel.tag);
    }
    let bytes = client()?
        .get(&rel.zip_url)
        .send()
        .with_context(|| format!("bajando {}", rel.zip_url))?
        .error_for_status()?
        .bytes()
        .context("leyendo el zip")?;

    // Verificar la firma minisign del zip (si `PUBLISHER_PUBKEY` esta seteada). Cierra el
    // vector "release/cuenta de GitHub comprometida sirve un binario malicioso". Con la clave
    // vacia (modo dev) NO verifica, igual que la sync.
    let sig = client()?
        .get(format!("{}.minisig", rel.zip_url))
        .send()
        .ok()
        .and_then(|r| r.error_for_status().ok())
        .and_then(|r| r.text().ok());
    crate::signing::verify_with_embedded(&bytes, sig.as_deref())
        .context("la firma del binario de update NO valida — se aborta la actualizacion")?;

    let cur = std::env::current_exe().context("current_exe")?;
    let tmp_exe = cur.with_extension("new");
    extract_named(&bytes, ASSET_EXE, &tmp_exe)?;

    // Limpiar el temp pase lo que pase (exito o si self_replace falla).
    let res = self_replace::self_replace(&tmp_exe).context("reemplazando el ejecutable");
    let _ = std::fs::remove_file(&tmp_exe);
    res?;

    std::process::Command::new(&cur)
        .spawn()
        .context("relanzando la app actualizada")?;
    std::process::exit(0);
}

/// Extrae del zip (en `bytes`) la entrada cuyo basename == `wanted` hacia `dest`.
fn extract_named(bytes: &[u8], wanted: &str, dest: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).context("zip invalido")?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let base = f
            .name()
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_string();
        if base.eq_ignore_ascii_case(wanted) {
            let mut out = std::fs::File::create(dest)
                .with_context(|| format!("creando {}", dest.display()))?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
            }
            return Ok(());
        }
    }
    bail!("el zip del release no contiene {wanted}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_compara_bien() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        // los sufijos -pre se ignoran (simplificacion): mismo core => no es mayor.
        assert!(!is_newer("0.1.0-rc1", "0.1.0"));
    }

    #[test]
    fn pick_best_release_ignora_drafts_y_tags_no_v() {
        let arr = vec![
            serde_json::json!({ "tag_name": "v0.2.0" }),
            serde_json::json!({ "tag_name": "2026.06.14" }), // set de mods -> ignorar
            serde_json::json!({ "tag_name": "v0.3.0", "draft": true }), // draft -> ignorar
            serde_json::json!({ "tag_name": "v0.2.3" }),     // este es el mayor v* no-draft
            serde_json::json!({ "tag_name": "v0.1.0" }),
            serde_json::json!({ "name": "sin tag" }),
        ];
        let best = pick_best_release(&arr).unwrap();
        assert_eq!(best.get("tag_name").unwrap().as_str().unwrap(), "v0.2.3");

        // Solo un release de set de mods (no-v) => None (no dispara update falso).
        let solo_mods = vec![serde_json::json!({ "tag_name": "2026.06.14" })];
        assert!(pick_best_release(&solo_mods).is_none());
    }

    #[test]
    fn release_from_json_extrae_tag_version_y_zip() {
        let v = serde_json::json!({
            "tag_name": "v0.2.3",
            "body": "notas",
            "html_url": "https://github.com/x/y/releases/tag/v0.2.3",
            "assets": [
                { "name": "leeme.txt", "browser_download_url": "https://x/leeme.txt" },
                { "name": "sts2-modsync-windows-x86_64.zip", "browser_download_url": "https://x/app.zip" }
            ]
        });
        let rel = release_from_json(&v).unwrap();
        assert_eq!(rel.tag, "v0.2.3");
        assert_eq!(rel.version, "0.2.3"); // sin la 'v'
        assert_eq!(rel.zip_url, "https://x/app.zip"); // primer asset .zip
        // sin tag => None.
        assert!(release_from_json(&serde_json::json!({ "body": "x" })).is_none());
    }

    #[test]
    fn extract_named_saca_por_basename() {
        // Arma un zip en memoria con el exe anidado y lo extrae por basename.
        let mut buf = Vec::new();
        {
            use std::io::Write;
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("otra.txt", opts).unwrap();
            zw.write_all(b"ruido").unwrap();
            zw.start_file("carpeta/sts2-modsync-gui.exe", opts).unwrap();
            zw.write_all(b"BINARIO-FALSO").unwrap();
            zw.finish().unwrap();
        }
        let dest = std::env::temp_dir().join("sts2_modsync_extract_test.exe");
        let _ = std::fs::remove_file(&dest);
        extract_named(&buf, ASSET_EXE, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"BINARIO-FALSO");
        // un nombre que no esta -> error.
        assert!(extract_named(&buf, "no-existe.exe", &dest).is_err());
        let _ = std::fs::remove_file(&dest);
    }
}
