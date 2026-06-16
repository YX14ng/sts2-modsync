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
    /// Version del formato. Se queda en 1: los campos NUEVOS son ADITIVOS (serde ignora los que no
    /// conoce), asi una app vieja sigue leyendo un codigo nuevo. Solo se subiria con un cambio que
    /// ROMPA (y entonces las apps viejas avisan "actualiza la app").
    v: u32,
    /// Nombre opcional de la lista (vacio = sin nombre).
    #[serde(default)]
    name: String,
    /// Ids de los mods que quedan HABILITADOS.
    on: Vec<String>,
    /// (Aditivo) versiones de esos mods (id, version), para diagnosticar diferencias de version al
    /// COMPARAR. NO entra en la huella (que es solo del orden de ids). Vacio en codigos viejos o
    /// cuando se comparte un perfil (que no guarda versiones).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ver: Vec<(String, String)>,
}

/// Lista decodificada de un codigo: ids HABILITADOS + (si el codigo las trae) sus versiones.
pub struct Decoded {
    pub name: String,
    pub on: Vec<String>,
    /// id -> version del que genero el codigo (vacio en codigos viejos / compartidos desde un perfil).
    pub versions: std::collections::HashMap<String, String>,
}

/// Codifica la lista de habilitados (+ nombre opcional) en un codigo compartible. Sin versiones (las
/// trae `encode_versioned`); sirve para compartir un perfil guardado, que no las almacena.
pub fn encode(name: &str, enabled_ids: &[String]) -> String {
    let with_none: Vec<(String, Option<String>)> =
        enabled_ids.iter().map(|id| (id.clone(), None)).collect();
    encode_versioned(name, &with_none)
}

/// Como `encode`, pero incluye la VERSION de cada mod (la del que genera el codigo) para que el que
/// compare pueda ver diferencias de version. `None` en la version = no se incluye para ese id.
pub fn encode_versioned(name: &str, enabled: &[(String, Option<String>)]) -> String {
    let on: Vec<String> = enabled.iter().map(|(id, _)| id.clone()).collect();
    let mut ver: Vec<(String, String)> = enabled
        .iter()
        .filter_map(|(id, v)| v.as_ref().map(|v| (id.clone(), v.clone())))
        .collect();
    ver.sort(); // determinismo del codigo (no depende del orden de entrada)
    let payload = Payload {
        v: 1,
        name: name.trim().to_string(),
        on,
        ver,
    };
    let json = serde_json::to_vec(&payload).unwrap_or_default();
    format!("{MAGIC}{}", URL_SAFE_NO_PAD.encode(deflate(&json)))
}

/// Decodifica un codigo a `(nombre, ids_habilitados)`. Tolera espacios/saltos de linea (un chat pudo
/// cortar la linea) y valida que los ids sean nombres simples. Error claro si no es un codigo valido.
pub fn decode(code: &str) -> Result<(String, Vec<String>)> {
    let d = decode_full(code)?;
    Ok((d.name, d.on))
}

/// Como `decode` pero ademas devuelve las VERSIONES que trae el codigo (para comparar). Las apps que
/// solo aplican usan `decode`; el "comparar con un amigo" usa este.
pub fn decode_full(code: &str) -> Result<Decoded> {
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
    let on: Vec<String> = payload
        .on
        .into_iter()
        .filter(|id| crate::manifest::is_simple_segment(id))
        .collect();
    // Solo quedan las versiones de ids que sobrevivieron el filtro.
    let versions: std::collections::HashMap<String, String> = payload
        .ver
        .into_iter()
        .filter(|(id, _)| on.contains(id))
        .collect();
    Ok(Decoded {
        name: payload.name,
        on,
        versions,
    })
}

/// Resultado de COMPARAR el estado local contra el codigo de un amigo: que difiere y si el orden de
/// carga (la huella) coincide.
pub struct Comparison {
    /// Huella del orden de carga LOCAL (los mods habilitados ahora).
    pub my_fingerprint: String,
    /// Huella del orden de carga del CODIGO (los ids que trae habilitados).
    pub their_fingerprint: String,
    /// `true` si las huellas coinciden: mismo orden de carga -> condicion (necesaria) para el mismo lobby.
    pub matches: bool,
    /// Ids del codigo que NO tenes instalados (hay que conseguirlos: instalar / sync). Bloquean el match.
    pub missing: Vec<String>,
    /// Ids del codigo que TENES pero DESHABILITADOS (activarlos te acerca al match).
    pub disabled: Vec<String>,
    /// Ids que tenes HABILITADOS y el codigo NO (sobran respecto del amigo).
    pub extra: Vec<String>,
    /// (id, tu_version, su_version) de mods que ambos tienen instalados pero con version distinta.
    pub version_diff: Vec<(String, String, String)>,
}

/// Compara los mods locales contra un codigo decodificado (el de un amigo) y explica QUE difiere para
/// entrar al mismo lobby. SOLO LECTURA: no cambia nada (se puede con el juego abierto).
pub fn compare(local_mods: &[crate::modlist::InstalledMod], code: &Decoded) -> Comparison {
    use std::collections::BTreeSet;
    let want: BTreeSet<&str> = code.on.iter().map(String::as_str).collect();
    let installed: BTreeSet<&str> = local_mods.iter().map(|m| m.id()).collect();
    let enabled: BTreeSet<&str> = local_mods
        .iter()
        .filter(|m| m.enabled)
        .map(|m| m.id())
        .collect();

    let missing: Vec<String> = code
        .on
        .iter()
        .filter(|id| !installed.contains(id.as_str()))
        .cloned()
        .collect();
    let disabled: Vec<String> = code
        .on
        .iter()
        .filter(|id| installed.contains(id.as_str()) && !enabled.contains(id.as_str()))
        .cloned()
        .collect();
    let extra: Vec<String> = enabled
        .iter()
        .filter(|id| !want.contains(*id))
        .map(|id| id.to_string())
        .collect();
    let mut version_diff = Vec::new();
    for m in local_mods {
        if let Some(their_v) = code.versions.get(m.id()) {
            let mine = m.manifest.version.as_deref().unwrap_or("");
            if !mine.is_empty() && mine != their_v {
                version_diff.push((m.id().to_string(), mine.to_string(), their_v.clone()));
            }
        }
    }
    version_diff.sort();

    let my_fingerprint = crate::modlist::current_fingerprint(local_mods);
    let their_fingerprint = crate::modlist::load_order_fingerprint(code.on.clone());
    Comparison {
        matches: my_fingerprint == their_fingerprint,
        my_fingerprint,
        their_fingerprint,
        missing,
        disabled,
        extra,
        version_diff,
    }
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
    fn compare_explica_que_difiere_y_la_huella() {
        use crate::modlist::InstalledMod;
        // Local: BaseLib (on), Extra (on), Char (off).
        let local = vec![
            InstalledMod::fake("BaseLib", true),
            InstalledMod::fake("Extra", true),
            InstalledMod::fake("Char", false),
        ];
        // Codigo del amigo: BaseLib, Char, Falta (Falta no esta instalado).
        let code = decode_full(&encode(
            "amigo",
            &["BaseLib".into(), "Char".into(), "Falta".into()],
        ))
        .unwrap();
        let c = compare(&local, &code);
        assert_eq!(c.missing, vec!["Falta".to_string()]); // no instalado
        assert_eq!(c.disabled, vec!["Char".to_string()]); // instalado pero off
        assert_eq!(c.extra, vec!["Extra".to_string()]); // on de mas
        assert!(!c.matches); // distinto conjunto -> distinta huella

        // Igualando el estado al codigo, las huellas coinciden y no hay diferencias.
        let local2 = vec![
            InstalledMod::fake("BaseLib", true),
            InstalledMod::fake("Char", true),
        ];
        let code2 = decode_full(&encode("x", &["BaseLib".into(), "Char".into()])).unwrap();
        let c2 = compare(&local2, &code2);
        assert!(c2.matches);
        assert!(c2.missing.is_empty() && c2.disabled.is_empty() && c2.extra.is_empty());
    }

    #[test]
    fn versiones_son_aditivas_y_compatibles() {
        // Un codigo CON versiones decodifica las versiones; `decode` (sin versiones) lo lee igual.
        let code = encode_versioned(
            "v",
            &[
                ("BaseLib".to_string(), Some("1.2.0".to_string())),
                ("Char".to_string(), None),
            ],
        );
        let d = decode_full(&code).unwrap();
        assert_eq!(d.on, vec!["BaseLib".to_string(), "Char".to_string()]);
        assert_eq!(d.versions.get("BaseLib").map(String::as_str), Some("1.2.0"));
        assert!(!d.versions.contains_key("Char"));
        // `decode` clasico sigue andando sobre el mismo codigo (compatibilidad).
        assert_eq!(decode(&code).unwrap().0, "v");
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
