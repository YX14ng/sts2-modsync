//! Delta binario intra-archivo (bsdiff, crate `qbsdiff`). Un patch transforma la version VIEJA de
//! un archivo (la que el cliente YA tiene en disco) en la NUEVA. Asi al actualizar un mod no se
//! rebaja el `.pck` ENTERO: solo el diff (cambiar una carta de un mod de 100 MB ~> un patch chico).
//!
//! SEGURIDAD — el delta es PURA OPTIMIZACION, nunca un vector de corrupcion:
//!  - El propio patch es un asset content-addressed: su BLAKE3 (`patch_blake3` del manifest) se
//!    verifica ANTES de aplicarlo, asi un patch adulterado no llega a ejecutarse.
//!  - `sync::apply` SIEMPRE verifica el BLAKE3 del RESULTADO contra el manifest; si el patch falla
//!    o el hash no coincide, baja el asset COMPLETO (fallback). Un delta malo no puede instalar bytes
//!    equivocados ni romper la transaccion.
//!  - `apply()` rechaza un patch cuyo header diga otro tamaño y ACOTA la salida a `expected_size`
//!    durante la materializacion (cota de memoria dura: un patch-bomba falla en vez de OOMear).

use anyhow::{Context, Result, bail};
use std::io::{Cursor, Write};

/// Genera un patch bsdiff que transforma `old` en `new` (lado modder / `publish`).
pub fn diff(old: &[u8], new: &[u8]) -> Result<Vec<u8>> {
    let mut patch = Vec::new();
    qbsdiff::Bsdiff::new(old, new)
        .compare(Cursor::new(&mut patch))
        .context("generando patch bsdiff")?;
    Ok(patch)
}

/// `Write` que CORTA (Err) en cuanto se escriben mas de `limit` bytes, sin crecer mas alla de eso.
/// Acota la salida de `bspatch` DURANTE la materializacion: un patch adulterado (controles que
/// escriben de mas, o un stream bzip2 que descomprime a GB) falla en O(limit) memoria en vez de
/// hacer crecer el buffer a gigabytes y OOMear el proceso. `bspatch.hint_target_size()` (el header)
/// lo controla el autor del patch, asi que NO alcanza con chequear el tamaño DESPUES de materializar.
struct CappedBuf {
    buf: Vec<u8>,
    limit: usize,
}

impl Write for CappedBuf {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.buf.len().saturating_add(data.len()) > self.limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "el patch intenta escribir mas que el tamaño esperado (patch invalido o malicioso)",
            ));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Aplica `patch` sobre `old` y devuelve la version nueva (lado cliente / `apply`). `expected_size`
/// es el tamaño del resultado segun el manifest. Se rechaza un patch cuyo header diga otro tamaño,
/// y la salida se acota a `expected_size` DURANTE la materializacion (cota de memoria dura contra un
/// patch-bomba). El llamador IGUAL debe verificar el BLAKE3 del resultado (esto solo cubre tamaño/RAM).
pub fn apply(old: &[u8], patch: &[u8], expected_size: u64) -> Result<Vec<u8>> {
    let bspatch = qbsdiff::Bspatch::new(patch).context("patch bsdiff invalido")?;
    // El header del patch (controlado por el autor) dice cuanto saldria: si no es EXACTO, rechazar
    // antes de tocar nada (patch para otra version, o uno mentido).
    let target = bspatch.hint_target_size();
    if target != expected_size {
        bail!("patch para otra version: target {target} bytes, esperaba {expected_size}");
    }
    let limit = usize::try_from(expected_size).context("expected_size no entra en usize")?;
    // Reservar modesto (no `limit` a ciegas: un `expected_size` mentido no debe forzar un alloc
    // enorme); el `CappedBuf` deja crecer hasta `limit` exacto y corta si el patch escribe de mas.
    let mut writer = CappedBuf {
        buf: Vec::with_capacity(limit.min(64 * 1024 * 1024)),
        limit,
    };
    bspatch
        .apply(old, &mut writer)
        .context("aplicando patch bsdiff (¿patch corrupto o malicioso?)")?;
    if writer.buf.len() as u64 != expected_size {
        bail!(
            "el patch produjo {} bytes, esperaba {expected_size}",
            writer.buf.len()
        );
    }
    Ok(writer.buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_apply_round_trip_y_patch_chico() {
        // Simula un `.pck`: bloque grande casi-identico, con un cambio chico y algo apendido.
        let old = b"the quick brown fox jumps over the lazy dog. ".repeat(2000);
        let mut new = old.clone();
        new[5000] = b'X';
        new[5001] = b'Y';
        new.extend_from_slice(b"... contenido nuevo apendido al final del archivo .pck");

        let p = diff(&old, &new).unwrap();
        // El patch tiene que ser MUCHO mas chico que el archivo nuevo completo (ese es el punto).
        assert!(
            p.len() * 4 < new.len(),
            "patch {} no es suficientemente chico vs new {}",
            p.len(),
            new.len()
        );

        let recon = apply(&old, &p, new.len() as u64).unwrap();
        assert_eq!(recon, new, "el patch no reconstruyo el archivo nuevo");
    }

    #[test]
    fn apply_rechaza_tamano_equivocado_y_patch_corrupto() {
        let old = b"datos viejos del archivo".to_vec();
        let new = b"datos nuevos del archivo, distintos".to_vec();
        let p = diff(&old, &new).unwrap();

        // Tamaño esperado equivocado -> error claro (no panic, no OOM).
        assert!(apply(&old, &p, 999_999).is_err());
        assert!(apply(&old, &p, (new.len() as u64) - 1).is_err());
        // Patch basura -> error (no panic).
        assert!(apply(&old, b"esto no es un patch bsdiff", new.len() as u64).is_err());
        // Patch correcto pero `old` equivocado -> reconstruye distinto; el tamaño coincide pero el
        // BLAKE3 (que verifica apply) NO: aca solo confirmamos que no panickea.
        let _ = apply(
            b"old totalmente distinto y de otro largo",
            &p,
            new.len() as u64,
        );
    }

    #[test]
    fn capped_buf_corta_al_pasar_el_limite() {
        let mut w = CappedBuf {
            buf: Vec::new(),
            limit: 4,
        };
        assert!(w.write_all(b"abc").is_ok()); // 3 <= 4
        assert!(w.write_all(b"de").is_err()); // 3+2 > 4 -> corta, NO crece sin limite
        assert!(
            w.buf.len() <= 4,
            "el buffer no debe crecer mas alla del limite"
        );
    }
}
