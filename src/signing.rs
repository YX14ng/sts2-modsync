//! Firma minisign. DOS modelos de confianza, segun lo que se firma:
//!  - **set-manifests (sync):** la firma es OPCIONAL (`verify_optional`). El ancla de confianza
//!    es que bajaste el manifest por HTTPS desde el repo del publicador que un amigo te paso, y
//!    cada asset se verifica por BLAKE3. Si el set TRAE firma se valida (capa extra) y una firma
//!    INVALIDA se rechaza (tampering); si no trae, se acepta como `Unsigned`. Asi un publicador
//!    no NECESITA manejar una clave minisign para compartir sets.
//!  - **binario de auto-update:** la firma es OBLIGATORIA (`verify_with_embedded`, estricto)
//!    porque baja y EJECUTA un binario; sin firma valida NO se actualiza.
//!
//! La firma garantiza AUTENTICIDAD e INTEGRIDAD (viene del publicador y no fue alterado), NO
//! inocuidad del codigo del mod. La clave PRIVADA jamas toca al cliente: vive en
//! `%APPDATA%/.../minisign.key` del modder. `PUBLISHER_PUBKEY` vacia = modo dev.

use anyhow::{Context, Result};
use std::io::Cursor;
use std::path::PathBuf;

/// Clave PUBLICA minisign del publicador (base64 cruda). Generar con `sts2-modsync keygen`
/// y pegar aca. Vacia = modo dev (sin verificar firma).
pub const PUBLISHER_PUBKEY: &str = "RWTJ1u2UXFr4U590zg+O8G1zSvC1f+Cdzfug9sNnL5s0CgOSOz0QSdLX";

/// Resultado de la verificacion (para mostrarlo de forma AFIRMATIVA en la UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigStatus {
    /// Firma valida verificada con la clave empotrada del publicador.
    Verified,
    /// El set NO trae firma: se confia en HTTPS + la URL del publicador (no es modo dev).
    Unsigned,
    /// Modo dev: `PUBLISHER_PUBKEY` vacia -> la firma NO se verifico.
    DevUnverified,
}

/// True si la verificacion de firma esta ACTIVA (hay una pubkey empotrada). False = modo dev.
pub fn is_enforced() -> bool {
    !PUBLISHER_PUBKEY.is_empty()
}

/// Verifica con la clave empotrada. Vacia => modo dev (`DevUnverified`, no falla). Con clave
/// seteada => exige firma valida y devuelve `Verified` (o `Err` si falta/no valida).
pub fn verify_with_embedded(manifest_bytes: &[u8], signature: Option<&str>) -> Result<SigStatus> {
    if PUBLISHER_PUBKEY.is_empty() {
        eprintln!("[seguridad] PUBLISHER_PUBKEY vacia: firma NO verificada (modo dev).");
        return Ok(SigStatus::DevUnverified);
    }
    let sig = signature.context("el set no trae firma y la verificacion es obligatoria")?;
    verify(PUBLISHER_PUBKEY, manifest_bytes, sig)?;
    Ok(SigStatus::Verified)
}

/// Verificacion OPCIONAL para SETS (sync). La firma minisign ya NO es obligatoria: el ancla de
/// confianza es que bajaste el manifest por HTTPS desde el repo del publicador que un amigo te
/// paso, y cada asset se verifica por BLAKE3. Reglas:
///  - el set TRAE firma valida -> `Verified` (capa extra de seguridad);
///  - el set TRAE firma pero NO valida -> `Err` (señal de tampering, se rechaza);
///  - el set NO trae firma -> `Unsigned` (se acepta, confiando en HTTPS + la URL);
///  - `PUBLISHER_PUBKEY` vacia -> `DevUnverified`.
///
/// OJO: el auto-update SIGUE exigiendo firma (`verify_with_embedded`, estricto) porque baja y
/// EJECUTA un binario; esto es solo para los set-manifests de sync.
pub fn verify_optional(manifest_bytes: &[u8], signature: Option<&str>) -> Result<SigStatus> {
    if PUBLISHER_PUBKEY.is_empty() {
        return Ok(SigStatus::DevUnverified);
    }
    match signature {
        Some(sig) => {
            verify(PUBLISHER_PUBKEY, manifest_bytes, sig)?;
            Ok(SigStatus::Verified)
        }
        None => Ok(SigStatus::Unsigned),
    }
}

/// Verifica `signature` (contenido de un `.minisig`) sobre `data` con `pubkey_b64`.
pub fn verify(pubkey_b64: &str, data: &[u8], signature: &str) -> Result<()> {
    let pk = minisign::PublicKey::from_base64(pubkey_b64)
        .map_err(|e| anyhow::anyhow!("clave publica minisign invalida: {e}"))?;
    let sig = minisign::SignatureBox::from_string(signature)
        .map_err(|e| anyhow::anyhow!("firma minisign invalida: {e}"))?;
    minisign::verify(&pk, &sig, Cursor::new(data), true, false, false)
        .map_err(|e| anyhow::anyhow!("la firma del manifiesto NO valida: {e}"))
}

/// Genera un par de claves SIN password. Devuelve (clave_secreta_box, clave_publica_base64):
/// la secreta es el contenido de un archivo de clave secreta minisign; la publica es la
/// base64 cruda para pegar en `PUBLISHER_PUBKEY`.
pub fn generate_keypair() -> Result<(String, String)> {
    let kp = minisign::KeyPair::generate_unencrypted_keypair()
        .map_err(|e| anyhow::anyhow!("generando claves: {e}"))?;
    let sk_box = kp
        .sk
        .to_box(None)
        .map_err(|e| anyhow::anyhow!("serializando clave secreta: {e}"))?
        .to_string();
    Ok((sk_box, kp.pk.to_base64()))
}

/// Firma `data` con la clave secreta (contenido de un archivo de clave secreta minisign sin
/// password). Devuelve el contenido del `.minisig`.
pub fn sign(secret_key_box: &str, data: &[u8]) -> Result<String> {
    let sk_box = minisign::SecretKeyBox::from_string(secret_key_box)
        .map_err(|e| anyhow::anyhow!("clave secreta minisign invalida: {e}"))?;
    let sk = minisign::SecretKey::from_unencrypted_box(sk_box)
        .map_err(|e| anyhow::anyhow!("clave secreta encriptada o invalida: {e}"))?;
    let sig = minisign::sign(
        None,
        &sk,
        Cursor::new(data),
        Some("sts2-modsync set-manifest"),
        None,
    )
    .map_err(|e| anyhow::anyhow!("firmando: {e}"))?;
    Ok(sig.to_string())
}

/// Ruta del archivo de clave secreta del modder (fuera del repo): `%APPDATA%/.../minisign.key`.
pub fn secret_key_path() -> Option<PathBuf> {
    Some(crate::config::config_path()?.parent()?.join("minisign.key"))
}

/// Lee la clave secreta del modder, si existe.
pub fn load_secret_key() -> Option<String> {
    std::fs::read_to_string(secret_key_path()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_round_trip() {
        let (sk, pk) = generate_keypair().unwrap();
        let data = b"contenido del set-manifest";
        let sig = sign(&sk, data).unwrap();
        assert!(verify(&pk, data, &sig).is_ok());
        // data distinta NO valida.
        assert!(verify(&pk, b"otra cosa", &sig).is_err());
    }

    #[test]
    fn verify_with_embedded_exige_firma_cuando_hay_pubkey() {
        // Con la verificacion ACTIVA (pubkey empotrada): un set SIN firma o con firma basura
        // debe rechazarse (no se baja un .dll sin autenticar). En modo dev no aplica.
        if !is_enforced() {
            eprintln!("(skip: PUBLISHER_PUBKEY vacia, modo dev)");
            return;
        }
        assert!(verify_with_embedded(b"data", None).is_err());
        assert!(verify_with_embedded(b"data", Some("no soy una firma")).is_err());
    }

    #[test]
    fn verify_optional_acepta_sin_firma_y_rechaza_firma_invalida() {
        if !is_enforced() {
            eprintln!("(skip: PUBLISHER_PUBKEY vacia, modo dev)");
            return;
        }
        // sin firma -> Unsigned (se acepta; el ancla es HTTPS + la URL).
        assert_eq!(verify_optional(b"data", None).unwrap(), SigStatus::Unsigned);
        // firma PRESENTE pero invalida -> rechaza (tampering).
        assert!(verify_optional(b"data", Some("no soy una firma")).is_err());
    }
}
