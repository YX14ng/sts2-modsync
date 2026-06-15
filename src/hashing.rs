//! Hashing BLAKE3 por archivo (la "capa delta gruesa": solo se baja lo que cambio).
//! BLAKE3 es ~6-12 GB/s y sin length-extension; con mmap+rayon hashea un .pck de
//! cientos de MB en paralelo sin cargarlo entero en RAM.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Hash BLAKE3 (hex) del contenido del archivo. Usa mmap+rayon (multihilo) y
/// cae a lectura en streaming si el mapeo falla.
pub fn blake3_file(path: &Path) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    // update_mmap_rayon mapea el archivo y hashea en paralelo; ideal para .pck grandes.
    match hasher.update_mmap_rayon(path) {
        Ok(_) => {}
        Err(_) => {
            // Fallback portable: streaming en bloques.
            use std::io::Read;
            let mut f = std::fs::File::open(path)
                .with_context(|| format!("no se pudo abrir {}", path.display()))?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = f
                    .read(&mut buf)
                    .with_context(|| format!("leyendo {}", path.display()))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Hash BLAKE3 (hex) de un buffer en memoria (p.ej. un patch bsdiff recien generado, o el
/// resultado de aplicar un patch antes de escribirlo a disco).
pub fn blake3_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Compara el hash de un archivo local contra el esperado (case-insensitive en hex).
/// `false` si el archivo no existe.
pub fn matches(path: &Path, expected_hex: &str) -> bool {
    if !path.is_file() {
        return false;
    }
    match blake3_file(path) {
        Ok(h) => h.eq_ignore_ascii_case(expected_hex),
        Err(_) => false,
    }
}

/// Tamano minimo para usar el cache. Los archivos CHICOS (los `.dll` de mods, < 8 MiB) se
/// re-hashean SIEMPRE: es barato y asi un `.dll` critico para el room-hash de multiplayer nunca
/// queda "al dia" por un hit stale. El cache solo evita re-hashear los `.pck` grandes.
const CACHE_MIN_SIZE: u64 = 8 * 1024 * 1024;

/// Entrada del cache: el blake3 se reusa mientras size+mtime no cambien (heuristica rsync). OJO:
/// si alguien reemplaza el contenido preservando AMBOS (p.ej. restore de una version vieja con
/// el mtime intacto), `plan()` puede marcar el archivo "al dia" y NO re-bajarlo — `apply`
/// re-verifica las DESCARGAS pero NO los archivos ya "al dia". Por eso los chicos no se cachean.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    size: u64,
    mtime_ns: u128,
    blake3: String,
}

/// Cache de hashes BLAKE3 por ruta: evita re-hashear los `.pck` de 100+ MB en cada `plan()`
/// cuando el archivo no cambio. Se persiste en `%APPDATA%/sts2-modsync/hashcache.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HashCache {
    #[serde(default)]
    entries: HashMap<String, CacheEntry>,
    #[serde(skip)]
    dirty: bool,
}

impl HashCache {
    /// Hash BLAKE3 (hex) de `path`, reusando el cache si size+mtime no cambiaron.
    pub fn blake3(&mut self, path: &Path) -> Result<String> {
        let meta = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
        let size = meta.len();
        // Archivos chicos: re-hashear siempre (barato; cierra el hit stale en los .dll criticos).
        if size < CACHE_MIN_SIZE {
            return blake3_file(path);
        }
        let mtime_ns = mtime_ns(&meta);
        let key = path.to_string_lossy().into_owned();
        if let Some(e) = self.entries.get(&key)
            && e.size == size
            && e.mtime_ns == mtime_ns
        {
            return Ok(e.blake3.clone());
        }
        let hash = blake3_file(path)?;
        self.entries.insert(
            key,
            CacheEntry {
                size,
                mtime_ns,
                blake3: hash.clone(),
            },
        );
        self.dirty = true;
        Ok(hash)
    }

    /// Como `matches` pero usando el cache. `false` si el archivo no existe o el stat falla.
    pub fn matches(&mut self, path: &Path, expected_hex: &str) -> bool {
        if !path.is_file() {
            return false;
        }
        match self.blake3(path) {
            Ok(h) => h.eq_ignore_ascii_case(expected_hex),
            Err(_) => false,
        }
    }

    /// Carga el cache de `%APPDATA%` (vacio si no existe o esta corrupto).
    pub fn load() -> Self {
        let Some(path) = cache_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persiste el cache si cambio (best-effort). Poda entradas de archivos que ya no existen
    /// para que no crezca sin limite.
    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        self.entries.retain(|k, _| Path::new(k).is_file());
        let Some(path) = cache_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(s) = serde_json::to_string(self) {
            let _ = std::fs::write(&path, s);
        }
        self.dirty = false;
    }
}

fn cache_path() -> Option<PathBuf> {
    Some(
        crate::config::config_path()?
            .parent()?
            .join("hashcache.json"),
    )
}

fn mtime_ns(meta: &std::fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashcache_cachea_grandes_revalida_y_saltea_chicos() {
        let dir = std::env::temp_dir().join("sts2_modsync_hashcache");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Archivo CHICO: se re-hashea siempre (bypass del cache) -> no marca dirty.
        let small = dir.join("small.bin");
        std::fs::write(&small, b"hola").unwrap();
        let mut c = HashCache::default();
        assert_eq!(c.blake3(&small).unwrap(), blake3_file(&small).unwrap());
        assert!(!c.dirty, "los archivos chicos no se cachean");

        // Archivo GRANDE (>= CACHE_MIN_SIZE): se cachea, hit en la 2da, revalida al cambiar.
        let big = dir.join("big.bin");
        std::fs::write(&big, vec![7u8; CACHE_MIN_SIZE as usize + 16]).unwrap();
        let h1 = c.blake3(&big).unwrap();
        assert_eq!(h1, blake3_file(&big).unwrap());
        assert!(c.dirty);
        assert_eq!(c.blake3(&big).unwrap(), h1); // hit, mismo valor

        std::fs::write(&big, vec![9u8; CACHE_MIN_SIZE as usize + 32]).unwrap(); // cambia size
        let h2 = c.blake3(&big).unwrap();
        assert_ne!(h1, h2);
        assert_eq!(h2, blake3_file(&big).unwrap());

        assert!(c.matches(&big, &h2));
        assert!(!c.matches(&big, &h1));
        assert!(!c.matches(&dir.join("noexiste"), &h2)); // archivo ausente -> false
        let _ = std::fs::remove_dir_all(&dir);
    }
}
