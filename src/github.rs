//! Integracion con GitHub para PUBLICAR sin el `gh` CLI (lado MODDER). Tres partes:
//!  - **login:** un Personal Access Token pegado, o el OAuth **device-flow** (si hay
//!    `OAUTH_CLIENT_ID`). El token se guarda SEGURO en el llavero del SO (Credential Manager
//!    en Windows) via `keyring`, nunca en texto plano.
//!  - **API REST:** crear el repo publico, crear el release y subir los assets.
//!  - El cliente que SINCRONIZA no usa nada de esto: baja del release publico por HTTPS.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Client ID de tu OAuth App de GitHub (para el device-flow). VACIO => device-flow deshabilitado
/// (se usa un token pegado). Para activarlo: registra una OAuth App (Settings -> Developer
/// settings -> OAuth Apps, tilda "Enable Device Flow"), pega aca su Client ID y recompila.
pub const OAUTH_CLIENT_ID: &str = "";

/// Scope minimo para crear repos PUBLICOS + releases + subir assets.
const OAUTH_SCOPE: &str = "public_repo";
const UA: &str = concat!("sts2-modsync/", env!("CARGO_PKG_VERSION"));
const API_VERSION: &str = "2022-11-28";
const KEYRING_SERVICE: &str = "sts2-modsync";
const KEYRING_USER: &str = "github-token";

// --- token storage (seguro: llavero del SO via keyring) ----------------------

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).context("abriendo el llavero del SO")
}

/// Guarda el token en el llavero del SO (Credential Manager en Windows).
pub fn store_token(token: &str) -> Result<()> {
    entry()?
        .set_password(token.trim())
        .context("guardando el token en el llavero")
}

/// Lee el token guardado, si hay.
pub fn load_token() -> Option<String> {
    entry().ok()?.get_password().ok()
}

/// Borra el token guardado (desconectar). No falla si no habia.
pub fn clear_token() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow::Error::new(e).context("borrando el token del llavero")),
    }
}

pub fn is_connected() -> bool {
    load_token().is_some()
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(UA)
        .build()
        .context("construir cliente http")
}

// --- OAuth device flow -------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default = "default_interval")]
    pub interval: u64,
    /// Segundos hasta que el `device_code` expira (para cortar el poll del lado cliente).
    #[serde(default = "default_expires")]
    pub expires_in: u64,
}
fn default_interval() -> u64 {
    5
}
fn default_expires() -> u64 {
    900 // ~15 min (default de GitHub)
}

/// Datos de un release (para crear/usar y subir assets).
#[derive(Deserialize)]
struct ReleaseInfo {
    id: u64,
    upload_url: String,
    html_url: String,
}

/// Resultado de un poll del device-flow.
pub enum DevicePoll {
    Pending,
    SlowDown,
    Token(String),
    Denied,
    Expired,
}

pub fn device_flow_enabled() -> bool {
    !OAUTH_CLIENT_ID.is_empty()
}

/// Arranca el device-flow: pide el codigo. Hay que mostrar `user_code` y abrir `verification_uri`,
/// y despues llamar `device_poll` cada `interval` segundos hasta tener token.
pub fn device_start() -> Result<DeviceCode> {
    if OAUTH_CLIENT_ID.is_empty() {
        bail!("device-flow no configurado (falta OAUTH_CLIENT_ID); pega un token");
    }
    client()?
        .post("https://github.com/login/device/code")
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[("client_id", OAUTH_CLIENT_ID), ("scope", OAUTH_SCOPE)])
        .send()
        .context("POST device/code")?
        .error_for_status()
        .context("github device/code")?
        .json::<DeviceCode>()
        .context("parseando device/code")
}

/// Un poll del token. Devuelve Pending mientras el usuario no autorizo todavia.
pub fn device_poll(device_code: &str) -> Result<DevicePoll> {
    #[derive(Deserialize)]
    struct Resp {
        access_token: Option<String>,
        error: Option<String>,
    }
    let resp: Resp = client()?
        .post("https://github.com/login/oauth/access_token")
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[
            ("client_id", OAUTH_CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .context("POST access_token")?
        .json()
        .context("parseando access_token")?;
    if let Some(t) = resp.access_token {
        return Ok(DevicePoll::Token(t));
    }
    Ok(match resp.error.as_deref() {
        Some("authorization_pending") => DevicePoll::Pending,
        Some("slow_down") => DevicePoll::SlowDown,
        Some("access_denied") => DevicePoll::Denied,
        Some("expired_token") => DevicePoll::Expired,
        other => bail!("device-flow: error inesperado {other:?}"),
    })
}

// --- API REST (crear repo / release / subir assets) --------------------------

pub struct Api {
    client: reqwest::blocking::Client,
    token: String,
}

impl Api {
    pub fn new(token: String) -> Self {
        Self {
            client: client().unwrap_or_else(|_| reqwest::blocking::Client::new()),
            token,
        }
    }

    fn req(&self, method: reqwest::Method, url: &str) -> reqwest::blocking::RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
    }

    /// Login del usuario autenticado (valida el token). GET /user.
    pub fn whoami(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct U {
            login: String,
        }
        let u: U = self
            .req(reqwest::Method::GET, "https://api.github.com/user")
            .send()
            .context("GET /user")?
            .error_for_status()
            .context("token de GitHub invalido o sin permiso")?
            .json()
            .context("parseando /user")?;
        Ok(u.login)
    }

    /// Crea el repo PUBLICO bajo el usuario autenticado si no existe (422 = ya existe -> ok).
    /// Para un repo de una ORG, crealo a mano: igual subimos al release si ya existe.
    pub fn ensure_repo(&self, name: &str) -> Result<()> {
        let resp = self
            .req(reqwest::Method::POST, "https://api.github.com/user/repos")
            .json(&serde_json::json!({ "name": name, "private": false, "auto_init": true }))
            .send()
            .context("POST /user/repos")?;
        match resp.status().as_u16() {
            201 | 422 => Ok(()), // creado, o ya existia
            code => {
                let body = resp.text().unwrap_or_default();
                bail!("no se pudo crear el repo {name:?} (HTTP {code}): {body}");
            }
        }
    }

    /// Sube `files` (nombre -> path) al release `tag` de `owner/repo`, creando el release si no
    /// existe y REEMPLAZANDO (clobber) los assets que ya estuvieran. Devuelve la URL del release.
    pub fn publish_assets(
        &self,
        owner: &str,
        repo: &str,
        tag: &str,
        files: &[(String, PathBuf)],
        mut on_progress: impl FnMut(usize, usize),
    ) -> Result<String> {
        let (release_id, upload_url, html_url) = self.get_or_create_release(owner, repo, tag)?;
        let existing = self.list_assets(owner, repo, release_id)?;
        let total = files.len();
        for (i, (name, path)) in files.iter().enumerate() {
            if let Some(asset_id) = existing.get(name) {
                self.delete_asset(owner, repo, *asset_id)?; // clobber segun el snapshot
            }
            let resp = self.upload_one(&upload_url, name, path)?;
            if resp.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
                // El asset aparecio entremedio (otro publish, o el snapshot quedo viejo): re-listar,
                // borrar el que choca y reintentar UNA vez (sino el 422 abortaria todo).
                if let Some(id) = self.list_assets(owner, repo, release_id)?.get(name) {
                    self.delete_asset(owner, repo, *id)?;
                }
                self.upload_one(&upload_url, name, path)?
                    .error_for_status()
                    .with_context(|| format!("subiendo {name} (reintento)"))?;
            } else {
                resp.error_for_status()
                    .with_context(|| format!("subiendo {name}"))?;
            }
            on_progress(i + 1, total);
        }
        Ok(html_url)
    }

    /// Un POST de subida del asset `name` (streamea el archivo, no lo carga entero en RAM).
    fn upload_one(
        &self,
        upload_url: &str,
        name: &str,
        path: &Path,
    ) -> Result<reqwest::blocking::Response> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("abriendo asset {}", path.display()))?;
        self.req(reqwest::Method::POST, &format!("{upload_url}?name={name}"))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(file)
            .send()
            .with_context(|| format!("subiendo {name}"))
    }

    /// (release_id, upload_url sin el template `{?name,label}`, html_url). Crea el release SOLO
    /// si el GET por tag da 404; cualquier otro error del GET (401/403/5xx) se PROPAGA (no se
    /// crea un release espurio). El create maneja el 422 'already_exists' con un re-GET.
    fn get_or_create_release(
        &self,
        owner: &str,
        repo: &str,
        tag: &str,
    ) -> Result<(u64, String, String)> {
        let get = self
            .req(
                reqwest::Method::GET,
                &format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/{tag}"),
            )
            .send()
            .context("GET release by tag")?;
        let status = get.status();
        let rel: ReleaseInfo = if status.is_success() {
            get.json().context("parseando release")?
        } else if status == reqwest::StatusCode::NOT_FOUND {
            self.create_release(owner, repo, tag)?
        } else {
            let body = get.text().unwrap_or_default();
            bail!("error consultando el release {tag} (HTTP {status}): {body}");
        };
        // upload_url viene como ".../assets{?name,label}" -> sacar el template.
        let upload = rel
            .upload_url
            .split('{')
            .next()
            .unwrap_or(&rel.upload_url)
            .to_string();
        Ok((rel.id, upload, rel.html_url))
    }

    /// Crea el release del tag; si choca con 422 (otro lo creo entremedio) hace re-GET por tag.
    fn create_release(&self, owner: &str, repo: &str, tag: &str) -> Result<ReleaseInfo> {
        let post = self
            .req(
                reqwest::Method::POST,
                &format!("https://api.github.com/repos/{owner}/{repo}/releases"),
            )
            .json(&serde_json::json!({
                "tag_name": tag,
                "name": tag,
                "body": "Set de mods publicado con sts2-modsync.",
            }))
            .send()
            .context("POST release")?;
        if post.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            return self
                .req(
                    reqwest::Method::GET,
                    &format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/{tag}"),
                )
                .send()
                .context("re-GET release tras 422")?
                .error_for_status()
                .context("re-GET release tras 422")?
                .json()
                .context("parseando release");
        }
        post.error_for_status()
            .context("creando el release (¿existe el repo y el token tiene permiso?)")?
            .json()
            .context("parseando release nuevo")
    }

    /// Mapa nombre-de-asset -> id, paginando (un set puede tener >100 assets).
    fn list_assets(
        &self,
        owner: &str,
        repo: &str,
        release_id: u64,
    ) -> Result<HashMap<String, u64>> {
        #[derive(Deserialize)]
        struct Asset {
            id: u64,
            name: String,
        }
        let mut out = HashMap::new();
        for page in 1.. {
            let assets: Vec<Asset> = self
                .req(
                    reqwest::Method::GET,
                    &format!(
                        "https://api.github.com/repos/{owner}/{repo}/releases/{release_id}/assets?per_page=100&page={page}"
                    ),
                )
                .send()
                .context("GET assets")?
                .error_for_status()
                .context("listando assets")?
                .json()
                .context("parseando assets")?;
            if assets.is_empty() {
                break;
            }
            let n = assets.len();
            out.extend(assets.into_iter().map(|a| (a.name, a.id)));
            if n < 100 {
                break;
            }
        }
        Ok(out)
    }

    fn delete_asset(&self, owner: &str, repo: &str, asset_id: u64) -> Result<()> {
        self.req(
            reqwest::Method::DELETE,
            &format!("https://api.github.com/repos/{owner}/{repo}/releases/assets/{asset_id}"),
        )
        .send()
        .context("DELETE asset")?
        .error_for_status()
        .context("borrando asset viejo")?;
        Ok(())
    }
}

/// Charset legal de un OWNER de GitHub (usuario/org): alfanumerico ASCII + guion.
fn valid_owner(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Charset legal de un REPO de GitHub: alfanumerico ASCII + `.` `_` `-`. Rechaza `.`/`..` (un `.`
/// es valido como caracter pero un segmento que ES `.` o contiene `..` seria path-traversal en el
/// `base_url` firmado: `github.com/owner/../releases/...`).
fn valid_repo(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Normaliza una entrada de repo a `"owner/repo"`. Acepta `owner/repo` o una URL de GitHub
/// (`https://github.com/owner/repo[/...]`, con o sin `.git`, `?query`/`#fragment` o `/releases/...`).
/// `None` si no salen dos segmentos VALIDOS (esto rechaza espacios internos, control-chars, CRLF,
/// `..`, `@host` y demas: el `owner/repo` termina en una URL/base_url firmada y en la API REST, asi
/// que tiene que estar limpio). El charset se valida contra las reglas reales de GitHub.
pub fn normalize_repo(input: &str) -> Option<String> {
    // Sacar query/fragment de una URL pegada del navegador (?tab=readme, #section) ANTES de partir.
    let s = input.trim().split(['?', '#']).next().unwrap_or("").trim();
    let s = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
        .unwrap_or(s);
    let mut parts = s
        .trim_start_matches('/')
        .split('/')
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?.trim_end_matches(".git");
    if valid_owner(owner) && valid_repo(repo) {
        Some(format!("{owner}/{repo}"))
    } else {
        None
    }
}

/// Valida un tag/version para NUESTRO esquema de URL (un solo segmento de path): no vacio, sin
/// espacios ni control-chars, sin `/` (rompe el round-trip con `parse_release_base_url`), sin `..`,
/// y con al menos un alfanumerico. El charset `[A-Za-z0-9._+-]` cubre SemVer completo (incluido el
/// build-metadata con `+`, que es seguro en un segmento de path). Devuelve el tag trimeado o `None`.
/// Se valida ANTES de armar el `base_url` porque ese texto queda EMBEBIDO (y firmado) en el
/// set-manifest que bajan los amigos.
pub fn valid_tag(version: &str) -> Option<String> {
    let v = version.trim();
    if v.is_empty() || v.contains("..") {
        return None;
    }
    let charset_ok = v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'));
    let has_alnum = v.chars().any(|c| c.is_ascii_alphanumeric());
    if charset_ok && has_alnum {
        Some(v.to_string())
    } else {
        None
    }
}

/// `base_url` de descarga del release para `owner/repo` + `version` (el tag, ya VALIDADO con
/// [`valid_tag`] / [`normalize_repo`] por el llamador): `https://github.com/<owner>/<repo>/releases/download/<version>/`.
pub fn release_base_url(repo: &str, version: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/{}/",
        repo.trim_matches('/'),
        version.trim()
    )
}

/// Deriva (owner, repo, tag) de un `base_url` de release de GitHub:
/// `https://github.com/<owner>/<repo>/releases/download/<tag>/`.
pub fn parse_release_base_url(base_url: &str) -> Option<(String, String, String)> {
    let rest = base_url.trim().strip_prefix("https://github.com/")?;
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    if parts.len() >= 5 && parts[2] == "releases" && parts[3] == "download" {
        Some((
            parts[0].to_string(),
            parts[1].to_string(),
            parts[4].to_string(),
        ))
    } else {
        None
    }
}

/// Junta los archivos a subir (nombre-de-asset -> path) de una carpeta de publicacion:
/// `set-manifest.json`, su `.minisig` (si esta), `set.torrent` (si esta), y `assets/*` (cada uno
/// nombrado por su blake3). El nombre del asset es el basename — el transporte baja por blake3.
pub fn collect_upload_files(out_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut add = |name: &str, p: PathBuf| {
        if p.is_file() {
            files.push((name.to_string(), p));
        }
    };
    add("set-manifest.json", out_dir.join("set-manifest.json"));
    add(
        "set-manifest.json.minisig",
        out_dir.join("set-manifest.json.minisig"),
    );
    add("set.torrent", out_dir.join("set.torrent"));
    if let Ok(rd) = std::fs::read_dir(out_dir.join("assets")) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file()
                && let Some(name) = p.file_name().and_then(|n| n.to_str())
            {
                files.push((name.to_string(), p));
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_release_base_url_ok_y_rechazo() {
        let (o, r, t) =
            parse_release_base_url("https://github.com/YX14ng/sts2-mods/releases/download/0.1/")
                .unwrap();
        assert_eq!(
            (o.as_str(), r.as_str(), t.as_str()),
            ("YX14ng", "sts2-mods", "0.1")
        );
        assert!(parse_release_base_url("https://example.com/x/").is_none());
        assert!(parse_release_base_url("https://github.com/u/r/").is_none()); // sin releases/download
    }

    #[test]
    fn normalize_repo_acepta_varias_formas() {
        let want = Some("YX14ng/sts2-mods".to_string());
        assert_eq!(normalize_repo("YX14ng/sts2-mods"), want);
        assert_eq!(
            normalize_repo("  https://github.com/YX14ng/sts2-mods "),
            want
        );
        assert_eq!(
            normalize_repo("https://github.com/YX14ng/sts2-mods/releases/download/v1/"),
            want
        );
        assert_eq!(
            normalize_repo("https://github.com/YX14ng/sts2-mods.git"),
            want
        );
        assert!(normalize_repo("soloesto").is_none());
        assert!(normalize_repo("").is_none());
        // round-trip: normalize -> release_base_url -> parse vuelve a owner/repo/tag.
        let base = release_base_url("YX14ng/sts2-mods", "2.0");
        assert_eq!(
            parse_release_base_url(&base),
            Some(("YX14ng".into(), "sts2-mods".into(), "2.0".into()))
        );
    }

    #[test]
    fn normalize_repo_limpia_y_rechaza_basura() {
        let want = Some("YX14ng/sts2-mods".to_string());
        // query/fragment de una URL pegada del navegador: se sacan.
        assert_eq!(normalize_repo("YX14ng/sts2-mods?tab=readme"), want);
        assert_eq!(
            normalize_repo("https://github.com/YX14ng/sts2-mods?tab=readme#x"),
            want
        );
        assert_eq!(normalize_repo("YX14ng/sts2-mods#readme"), want);
        // espacios alrededor de los segmentos: se trimean.
        assert_eq!(normalize_repo("  YX14ng / sts2-mods "), want);
        // el repo de la APP (distinto del de sets) tambien se acepta por la misma regla.
        assert_eq!(
            normalize_repo("YX14ng/sts2-modsync"),
            Some("YX14ng/sts2-modsync".to_string())
        );
        // basura que ANTES pasaba como owner/repo roto: ahora None.
        assert!(normalize_repo("ow ner/re po").is_none()); // espacio interno
        assert!(normalize_repo("evil.com/owner/repo").is_none()); // owner con '.'
        assert!(normalize_repo("@evil.com/owner").is_none()); // owner con '@'
        assert!(normalize_repo("../..").is_none()); // path traversal
        assert!(normalize_repo("owner/..").is_none()); // '..' en el segmento repo
        assert!(normalize_repo("owner/.").is_none()); // '.' como repo
        assert!(normalize_repo("owner/a..b").is_none()); // '..' embebido en el repo
        assert!(normalize_repo("owner/re\rpo").is_none()); // control char (CR)
        assert!(normalize_repo("owner/repo:8080").is_none()); // ':' no es charset de repo
    }

    #[test]
    fn valid_tag_acepta_tags_normales_y_rechaza_los_que_rompen_la_url() {
        assert_eq!(valid_tag("v1.2.3"), Some("v1.2.3".into()));
        assert_eq!(valid_tag("2026.06.14"), Some("2026.06.14".into()));
        assert_eq!(valid_tag("1.0.0-rc.1"), Some("1.0.0-rc.1".into())); // SemVer pre-release
        assert_eq!(valid_tag("v1.0.0+build.5"), Some("v1.0.0+build.5".into())); // SemVer +build
        assert_eq!(valid_tag("  1.0.0 "), Some("1.0.0".into())); // trim de extremos
        assert!(valid_tag("").is_none());
        assert!(valid_tag("   ").is_none());
        assert!(valid_tag("release/1.0").is_none()); // '/' romperia el round-trip del base_url
        assert!(valid_tag("my version").is_none()); // espacio interno -> URL invalida
        assert!(valid_tag("v1\r\nHost: evil").is_none()); // CRLF
        assert!(valid_tag("..").is_none()); // traversal
        assert!(valid_tag("...").is_none()); // sin alfanumerico
        assert!(valid_tag("-").is_none()); // sin alfanumerico
    }
}
