//! Contrato de transporte (descarga). El resto del codigo depende de esta
//! abstraccion, NO de reqwest, para que cambiar de fuente (GitHub Releases, R2,
//! mirror local) sea contenido. La implementacion concreta es FASE 2 —
//! ver HANDOFF.md §transporte.

use crate::manifest::FileEntry;
use anyhow::Result;
use std::path::Path;

/// Una fuente desde la que bajar los archivos de un set.
pub trait ModSource {
    /// Descarga `entry.path` (resuelto contra `base_url`) hacia `dest`, idealmente
    /// reanudable, y DEBE verificar que el BLAKE3 del resultado coincida con
    /// `entry.blake3` antes de considerarlo valido.
    fn fetch(&self, base_url: &str, entry: &FileEntry, dest: &Path) -> Result<()>;
}

/// Fuente recomendada: assets de un GitHub Release, bajados por su
/// `browser_download_url` directa (NO via la REST API, para esquivar el
/// rate-limit anonimo de 60 req/h de api.github.com). Gratis, CDN, sin login.
///
/// FASE 2: implementar con `reqwest` 0.12 (rustls) + HTTP Range para reanudar,
/// reportando progreso por un canal hacia la GUI.
pub struct GitHubReleases;

impl ModSource for GitHubReleases {
    fn fetch(&self, _base_url: &str, _entry: &FileEntry, _dest: &Path) -> Result<()> {
        anyhow::bail!("GitHubReleases::fetch es FASE 2 (reqwest + Range) — ver HANDOFF.md")
    }
}
