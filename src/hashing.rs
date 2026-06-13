//! Hashing BLAKE3 por archivo (la "capa delta gruesa": solo se baja lo que cambio).
//! BLAKE3 es ~6-12 GB/s y sin length-extension; con mmap+rayon hashea un .pck de
//! cientos de MB en paralelo sin cargarlo entero en RAM.

use anyhow::{Context, Result};
use std::path::Path;

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
