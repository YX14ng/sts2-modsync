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
    /// Descarga `entry.path` (resuelto contra `base_url`) hacia `dest`, llamando `on_bytes`
    /// con la cantidad de bytes NUEVOS de cada chunk (para la barra de progreso). NO
    /// verifica el hash: eso lo hace `sync::apply` tras bajar (separa transporte de
    /// verificacion, y apply ya tiene `hashing`).
    fn fetch(
        &self,
        base_url: &str,
        entry: &FileEntry,
        dest: &Path,
        on_bytes: &mut dyn FnMut(u64),
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
        on_bytes: &mut dyn FnMut(u64),
    ) -> Result<()> {
        // Content-addressed: el asset remoto se llama por su BLAKE3 (no por la ruta local).
        let url = join_url(base_url, &entry.blake3);

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
            on_bytes(n as u64);
        }
        file.flush().ok();

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

/// GET de una URL y devuelve el body como texto. Para bajar el `set-manifest.json` desde
/// una URL (p.ej. el asset de un GitHub Release) en vez de un archivo local.
pub fn get_text(url: &str) -> Result<String> {
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

/// Une `base` + `path` relativo con una sola `/`.
fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}
