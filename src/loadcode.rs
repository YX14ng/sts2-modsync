//! Codigo COMPARTIBLE de la "lista de carga" (que mods quedan HABILITADOS / deshabilitados). Un
//! amigo que YA tiene los mods instalados pega el codigo y la app habilita los del codigo y
//! deshabilita el resto — el orden de carga canonico (BaseLib + A-Z, via ModListSorter) sale solo,
//! asi entran al MISMO lobby. NO baja archivos (eso es la sync): solo comparte el estado on/off.
//!
//! Formato AUTOCONTENIDO (sin servidor): `STS2L1.` + base64url(deflate(JSON `{v,name,on}`)). La
//! deflate lo achica (los ids comparten estructura) y base64url evita `+`/`/` que algunos chats
//! mastican. El que aplica reusa `profile::apply` (habilita el set, deshabilita el resto).

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// Prefijo que identifica un codigo de lista (v1). El `.` separa el magic del payload base64url.
const MAGIC: &str = "STS2L1.";

#[derive(Serialize, Deserialize)]
struct Payload {
    /// Version del formato (para evolucionar sin romper codigos viejos).
    v: u32,
    /// Nombre opcional de la lista (vacio = sin nombre).
    #[serde(default)]
    name: String,
    /// Ids de los mods que quedan HABILITADOS.
    on: Vec<String>,
}

/// Codifica la lista de habilitados (+ nombre opcional) en un codigo compartible.
pub fn encode(name: &str, enabled_ids: &[String]) -> String {
    let payload = Payload {
        v: 1,
        name: name.trim().to_string(),
        on: enabled_ids.to_vec(),
    };
    let json = serde_json::to_vec(&payload).unwrap_or_default();
    format!("{MAGIC}{}", URL_SAFE_NO_PAD.encode(deflate(&json)))
}

/// Decodifica un codigo a `(nombre, ids_habilitados)`. Tolera espacios/saltos de linea (un chat pudo
/// cortar la linea) y valida que los ids sean nombres simples. Error claro si no es un codigo valido.
pub fn decode(code: &str) -> Result<(String, Vec<String>)> {
    let body = code
        .trim()
        .strip_prefix(MAGIC)
        .context("no es un codigo de lista de sts2-modsync (deberia empezar con STS2L1.)")?;
    // Sacar cualquier whitespace que un chat haya metido en el medio.
    let cleaned: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let compressed = URL_SAFE_NO_PAD
        .decode(cleaned.as_bytes())
        .context("el codigo esta corrupto (base64 invalido)")?;
    let json = inflate(&compressed).context("el codigo esta corrupto (no se pudo descomprimir)")?;
    let payload: Payload =
        serde_json::from_slice(&json).context("el codigo no tiene un formato valido")?;
    if payload.v != 1 {
        bail!(
            "este codigo es de una version mas nueva ({}): actualiza la app",
            payload.v
        );
    }
    // Higiene: descartar ids que no sean nombres de carpeta simples (nunca arman paths aca, pero
    // viajan a `manager` que igual los valida; filtrarlos evita ruido).
    let ids: Vec<String> = payload
        .on
        .into_iter()
        .filter(|id| crate::manifest::is_simple_segment(id))
        .collect();
    Ok((payload.name, ids))
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// Tope de lo que un codigo puede descomprimir. El codigo es UNTRUSTED (lo pega un amigo): sin tope,
/// un payload muy comprimible (zip-bomb) podria reventar la RAM. Una lista de miles de ids entra de
/// sobra en 1 MB.
const MAX_INFLATED: u64 = 1024 * 1024;

fn inflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    // `take(MAX+1)`: corta la descompresion en cuanto se pasa del tope (no materializa el bomb entero).
    flate2::read::DeflateDecoder::new(data)
        .take(MAX_INFLATED + 1)
        .read_to_end(&mut out)
        .context("inflate")?;
    if out.len() as u64 > MAX_INFLATED {
        bail!("el codigo descomprime a algo enorme (¿corrupto o malicioso?)");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_codigo() {
        let ids = vec![
            "BaseLib".to_string(),
            "FGOCore".to_string(),
            "ModListSorter".to_string(),
        ];
        let code = encode("Mi Lista", &ids);
        assert!(code.starts_with("STS2L1."));
        let (name, back) = decode(&code).unwrap();
        assert_eq!(name, "Mi Lista");
        assert_eq!(back, ids);
        // tolera whitespace alrededor y en el medio (un chat que corta la linea).
        let chopped = format!("  {}\n{}  ", &code[..20], &code[20..]);
        assert_eq!(decode(&chopped).unwrap().1, ids);
    }

    #[test]
    fn rechaza_basura_y_filtra_ids_no_simples() {
        assert!(decode("no soy un codigo").is_err());
        assert!(decode("STS2L1.@@@no-base64@@@").is_err());
        // un id con separador (`..`/`/`) se filtra al decodificar (defensa).
        let code = encode("", &["BaseLib".into(), "../evil".into(), "a/b".into()]);
        assert_eq!(decode(&code).unwrap().1, vec!["BaseLib".to_string()]);
    }

    #[test]
    fn rechaza_zip_bomb() {
        // Un codigo chico que descomprime a > MAX_INFLATED se rechaza (no revienta la RAM).
        let big = vec![b'a'; (MAX_INFLATED + 4096) as usize];
        let code = format!("{MAGIC}{}", URL_SAFE_NO_PAD.encode(deflate(&big)));
        let err = format!("{:#}", decode(&code).unwrap_err());
        assert!(err.contains("enorme"), "deberia cortar por el tope: {err}");
    }
}
