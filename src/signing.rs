//! Firma minisign del set-manifest. P0 de seguridad: el cliente lleva la clave PUBLICA del
//! publicador empotrada (`PUBLISHER_PUBKEY`) y rechaza todo manifest cuya firma no valide.
//! Esto cierra "un atacante sustituye el .dll/manifest en el hosting o hace MITM".
//!
//! OJO: la firma garantiza AUTENTICIDAD e INTEGRIDAD (viene del publicador y no fue
//! alterado), NO inocuidad del codigo del mod — el amigo igual confia en el publicador.
//! La clave PRIVADA jamas toca al cliente: vive en `%APPDATA%/.../minisign.key` del modder.
//!
//! `PUBLISHER_PUBKEY` vacia = **modo dev** (firma NO verificada). Para produccion: correr
//! `sts2-modsync keygen`, pegar la clave publica aca, y recompilar.

use anyhow::{Context, Result};
use std::io::Cursor;
use std::path::PathBuf;

/// Clave PUBLICA minisign del publicador (base64 cruda). Generar con `sts2-modsync keygen`
/// y pegar aca. Vacia = modo dev (sin verificar firma).
pub const PUBLISHER_PUBKEY: &str = "RWTJ1u2UXFr4U590zg+O8G1zSvC1f+Cdzfug9sNnL5s0CgOSOz0QSdLX";

/// Verifica con la clave empotrada. Vacia => modo dev (no falla). Con clave seteada => exige
/// que el set traiga firma valida.
pub fn verify_with_embedded(manifest_bytes: &[u8], signature: Option<&str>) -> Result<()> {
    if PUBLISHER_PUBKEY.is_empty() {
        eprintln!("[seguridad] PUBLISHER_PUBKEY vacia: firma NO verificada (modo dev).");
        return Ok(());
    }
    let sig = signature.context("el set no trae firma y la verificacion es obligatoria")?;
    verify(PUBLISHER_PUBKEY, manifest_bytes, sig)
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
}
