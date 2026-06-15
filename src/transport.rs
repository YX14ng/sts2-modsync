//! Transporte (descarga). El resto del codigo depende de esta abstraccion (`ModSource`),
//! NO de reqwest, para que cambiar de fuente (GitHub Releases, R2, mirror local) sea
//! contenido. La impl concreta usa **reqwest blocking** (rustls): la descarga corre en el
//! worker thread del GUI / CLI, asi que no hace falta async/tokio.

use crate::manifest::FileEntry;
use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::path::Path;

/// Una fuente desde la que bajar los archivos de un set.
pub trait ModSource {
    /// Pre-carga (opcional) TODO el conjunto `entries` de una, antes del loop de `fetch`.
    /// Pensado para backends que bajan el set entero a la vez (p.ej. un torrent: se une al
    /// swarm y baja los archivos seleccionados juntos). `on_bytes` recibe los bytes NUEVOS
    /// a medida que llegan (para la barra). Default = no-op: las fuentes por-archivo (HTTP)
    /// no necesitan esto y bajan en `fetch`. Si `prepare` deja un archivo cacheado, `fetch`
    /// NO debe volver a contar esos bytes (los reporto aca). `on_bytes(n) -> bool`: devuelve
    /// `false` para CANCELAR (el backend debe abortar limpio en cuanto lo vea).
    fn prepare(
        &self,
        _entries: &[FileEntry],
        _on_bytes: &mut dyn FnMut(u64) -> bool,
    ) -> Result<()> {
        Ok(())
    }

    /// Descarga `entry.path` (resuelto contra `base_url`) hacia `dest`, llamando `on_bytes`
    /// con la cantidad de bytes NUEVOS de cada chunk (para la barra de progreso). NO
    /// verifica el hash: eso lo hace `sync::apply` tras bajar (separa transporte de
    /// verificacion, y apply ya tiene `hashing`). Si `prepare` ya dejo el archivo listo, la
    /// impl puede moverlo y reportar 0 bytes (ya contados en `prepare`). `on_bytes(n) -> bool`
    /// devuelve `false` para CANCELAR: la impl debe cortar la descarga y devolver `Err`.
    fn fetch(
        &self,
        base_url: &str,
        entry: &FileEntry,
        dest: &Path,
        on_bytes: &mut dyn FnMut(u64) -> bool,
    ) -> Result<()>;
}

/// Fuente recomendada: assets de un GitHub Release, bajados por su `browser_download_url`
/// directa (NO via la REST API, para esquivar el rate-limit anonimo de 60 req/h de
/// api.github.com). Gratis, CDN, sin login.
///
/// **Esquema content-addressed:** los assets de un Release son PLANOS (sin carpetas) y
/// GitHub sanitiza nombres raros, asi que NO se puede subir "BaseLib/BaseLib.dll". El
/// nombre del asset es el **BLAKE3** del archivo (`entry.blake3`): hex seguro, sin
/// colisiones, con dedup. `entry.path` queda SOLO para la ruta local de instalacion.
pub struct GitHubReleases {
    client: reqwest::blocking::Client,
}

impl GitHubReleases {
    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!("sts2-modsync/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("construir cliente reqwest");
        Self { client }
    }
}

impl Default for GitHubReleases {
    fn default() -> Self {
        Self::new()
    }
}

impl ModSource for GitHubReleases {
    fn fetch(
        &self,
        base_url: &str,
        entry: &FileEntry,
        dest: &Path,
        on_bytes: &mut dyn FnMut(u64) -> bool,
    ) -> Result<()> {
        // Content-addressed: el asset remoto se llama por su BLAKE3 (no por la ruta local).
        let url = join_url(base_url, &entry.blake3);
        // HTTPS obligatorio tambien para los assets (.dll/.pck que el juego ejecuta), no solo
        // el manifest. El base_url ya se valida al parsear, pero esto cubre cada GET.
        require_https(&url)?;

        // ¿Reanudar? Si el `.part` quedo a medias de un intento previo, pedimos solo el resto
        // con HTTP Range (los `.pck` de 100+ MB no se rehacen desde cero).
        let existing = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        let want_resume = entry.size != 0 && existing > 0 && existing < entry.size;

        let mut req = self.client.get(&url);
        if want_resume {
            req = req.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }
        let resp = req
            .send()
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("descargando {} ({})", entry.path, entry.blake3))?;

        // 206 = el server respeto el Range (append); si no (200), bajamos de cero (truncate).
        let resumed = want_resume && resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut file = if resumed {
            std::fs::OpenOptions::new()
                .append(true)
                .open(dest)
                .with_context(|| format!("reabriendo {}", dest.display()))?
        } else {
            std::fs::File::create(dest).with_context(|| format!("creando {}", dest.display()))?
        };
        if resumed {
            on_bytes(existing); // contar lo ya bajado para la barra de progreso
        }

        let mut reader = resp;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf).context("leyendo del servidor")?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).context("escribiendo a disco")?;
            if !on_bytes(n as u64) {
                bail!("descarga cancelada");
            }
        }
        file.flush().context("flush a disco")?;

        // Sanity de tamaño (el hash lo chequea apply). Atrapa "asset equivocado/faltante" o
        // un 404 que devolvio HTML, antes de gastar el hash.
        let final_size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        if entry.size != 0 && final_size != entry.size {
            bail!(
                "{}: tamaño final {final_size} bytes, esperaba {} (¿asset equivocado o faltante?)",
                entry.path,
                entry.size
            );
        }
        Ok(())
    }
}

/// Resuelve la URL del `set-manifest.json` del **ULTIMO release** de un repo PUBLICO de GitHub.
/// Consulta `GET /repos/{owner}/{repo}/releases/latest` (sin login: el rate-limit anonimo de
/// 60 req/h alcanza de sobra para un chequeo manual) y arma
/// `https://github.com/<owner>/<repo>/releases/download/<tag>/set-manifest.json`. Asi una
/// suscripcion por REPO sigue el release mas nuevo sin tener que re-pegar la URL en cada update.
/// `releases/latest` excluye drafts y pre-releases (lo que GitHub considera "el ultimo").
pub fn resolve_latest_manifest(owner: &str, repo: &str) -> Result<String> {
    let api = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("sts2-modsync/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("construir cliente http")?;
    let resp = client
        .get(&api)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .with_context(|| format!("GET {api}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        bail!(
            "el repo {owner}/{repo} no tiene releases publicados todavia (¿ya publicaste un set?)"
        );
    }
    let body = resp
        .error_for_status()
        .context("github api (releases/latest)")?
        .text()?;
    manifest_url_from_latest(owner, repo, &body)
}

/// Parsea el JSON de `releases/latest` y arma la URL del `set-manifest.json` de ese release.
/// Helper puro (testeable sin red) de [`resolve_latest_manifest`]. El `tag` se valida con
/// `github::valid_tag` (la MISMA regla que el lado publish): rechaza `/`, `..`, espacios y demas
/// que romperian la URL, en vez de interpolar texto crudo en el path.
fn manifest_url_from_latest(owner: &str, repo: &str, body: &str) -> Result<String> {
    let v: serde_json::Value =
        serde_json::from_str(body).context("json invalido de releases/latest")?;
    let raw = v
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let tag = crate::github::valid_tag(raw).with_context(|| {
        format!("el ultimo release de {owner}/{repo} tiene un tag invalido para la URL: {raw:?}")
    })?;
    Ok(format!(
        "https://github.com/{owner}/{repo}/releases/download/{tag}/set-manifest.json"
    ))
}

/// GET de una URL y devuelve el body como texto. Para bajar el `set-manifest.json` desde
/// una URL (p.ej. el asset de un GitHub Release) en vez de un archivo local.
pub fn get_text(url: &str) -> Result<String> {
    require_https(url)?; // el manifest/.minisig no se bajan en claro
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("sts2-modsync/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("construir cliente http")?;
    let body = client
        .get(url)
        .send()
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("bajando {url}"))?
        .text()?;
    Ok(body)
}

/// Rechaza URLs `http://` (defensa en profundidad): manifest, firma Y assets (.dll/.pck que el
/// juego ejecuta) se bajan SIEMPRE por HTTPS. EXCEPCION: `http://` a LOOPBACK
/// (127.0.0.1 / localhost / [::1]) se permite — ese trafico no sale de la maquina, no hay MITM
/// posible, y habilita mirrors/tests locales. Una base local/relativa (sin `http://`) tambien pasa.
pub fn require_https(url: &str) -> Result<()> {
    let lower = url.trim_start().to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("http://")
        && !is_loopback_host(rest)
    {
        bail!("URL insegura (http://): se exige HTTPS — {url}");
    }
    Ok(())
}

/// True si lo que sigue a `http://` apunta a un host de loopback EXACTO (seguido de `:`puerto,
/// `/`ruta o fin). Evita el bypass tipo `127.0.0.1.evil.com`.
fn is_loopback_host(after_scheme: &str) -> bool {
    let host_port = after_scheme.split('/').next().unwrap_or("");
    for h in ["127.0.0.1", "localhost", "[::1]"] {
        if let Some(tail) = host_port.strip_prefix(h)
            && (tail.is_empty() || tail.starts_with(':'))
        {
            return true;
        }
    }
    false
}

/// Une `base` + `path` relativo con una sola `/`.
fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_url_normaliza_la_barra() {
        assert_eq!(join_url("https://x/", "abc"), "https://x/abc");
        assert_eq!(join_url("https://x", "abc"), "https://x/abc");
        assert_eq!(join_url("https://x/", "/abc"), "https://x/abc");
        assert_eq!(join_url("https://x///", "//abc"), "https://x/abc");
    }

    #[test]
    fn manifest_url_from_latest_arma_la_url_del_ultimo_release() {
        let body = r#"{"tag_name":"2026.06.20","name":"set","draft":false}"#;
        assert_eq!(
            manifest_url_from_latest("YX14ng", "sts2-mods", body).unwrap(),
            "https://github.com/YX14ng/sts2-mods/releases/download/2026.06.20/set-manifest.json"
        );
        // sin tag -> error claro, no una URL rota.
        assert!(manifest_url_from_latest("o", "r", r#"{"name":"x"}"#).is_err());
        assert!(manifest_url_from_latest("o", "r", r#"{"tag_name":""}"#).is_err());
        // tag con '/' (rompe el round-trip del path) -> rechazado por valid_tag, no URL rota.
        assert!(manifest_url_from_latest("o", "r", r#"{"tag_name":"release/1.0"}"#).is_err());
        // json invalido -> error (no panic).
        assert!(manifest_url_from_latest("o", "r", "no json").is_err());
    }

    #[test]
    fn get_text_rechaza_http_inseguro() {
        // Falla ANTES de tocar la red (la verificacion de http:// es lo primero).
        assert!(get_text("http://example/set-manifest.json").is_err());
    }

    #[test]
    fn require_https_rechaza_http_salvo_loopback() {
        assert!(require_https("http://example/x").is_err());
        assert!(require_https("  HTTP://EXAMPLE/x").is_err()); // case/espacios
        assert!(require_https("https://example/x").is_ok());
        assert!(require_https("set-manifest.json").is_ok()); // base local/relativa (tests)
        // loopback http:// permitido (no hay MITM); pero el bypass tipo 127.0.0.1.evil NO.
        assert!(require_https("http://127.0.0.1:8080/a").is_ok());
        assert!(require_https("http://localhost/a").is_ok());
        assert!(require_https("http://[::1]:9/a").is_ok());
        assert!(require_https("http://127.0.0.1.evil.com/a").is_err());
        assert!(require_https("http://localhost.evil.com/a").is_err());
    }

    /// Mock loopback: verifica que `GitHubReleases::fetch` baja entero (200) y REANUDA con
    /// Range (206) completando el `.part`, y que respeta el tamano final esperado.
    #[test]
    fn fetch_full_200_y_resume_206_contra_un_mock() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let body: &[u8] = b"0123456789ABCDEF"; // 16 bytes conocidos
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body_vec = body.to_vec();
        let server = std::thread::spawn(move || {
            // Atiende 2 conexiones: full (sin Range) y resume (con Range).
            for _ in 0..2 {
                let (mut sock, _) = listener.accept().unwrap();
                let mut buf = [0u8; 2048];
                let n = sock.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..n]);
                let range_start = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|l| l.split('=').nth(1))
                    .and_then(|s| s.trim().trim_end_matches('-').parse::<usize>().ok());
                let (status, part) = match range_start {
                    Some(start) => ("206 Partial Content", &body_vec[start..]),
                    None => ("200 OK", &body_vec[..]),
                };
                // `Connection: close` -> el cliente NO reusa el socket (2 conexiones reales).
                let hdr = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    part.len()
                );
                sock.write_all(hdr.as_bytes()).unwrap();
                sock.write_all(part).unwrap();
                let _ = sock.flush();
            }
        });

        let base = format!("http://127.0.0.1:{port}/");
        let entry = FileEntry {
            path: "Mod/a.bin".into(),
            size: body.len() as u64,
            blake3: "00".into(), // el hash lo verifica apply, no transport
            deltas: Vec::new(),
        };
        let dest = std::env::temp_dir().join(format!("sts2_transport_mock_{port}.part"));
        let _ = std::fs::remove_file(&dest);
        let src = GitHubReleases::new();

        // 1) Sin `.part` -> baja entero (200).
        let mut got = 0u64;
        src.fetch(&base, &entry, &dest, &mut |n| {
            got += n;
            true
        })
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(got, body.len() as u64);

        // 2) `.part` parcial (6 bytes) -> pide Range, server responde 206, completa.
        std::fs::write(&dest, &body[..6]).unwrap();
        src.fetch(&base, &entry, &dest, &mut |_| true).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);

        server.join().unwrap();
        let _ = std::fs::remove_file(&dest);
    }
}
