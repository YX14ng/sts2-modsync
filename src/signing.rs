//! Verificacion de la firma minisign del manifiesto. Es la mitigacion P0 de
//! seguridad: el cliente lleva la clave PUBLICA del publicador empotrada
//! (pinning/TOFU) y rechaza todo manifiesto cuya firma no valide. Esto cierra el
//! vector "atacante sustituye el .dll/manifiesto en el hosting o hace MITM".
//!
//! OJO: la firma garantiza AUTENTICIDAD e INTEGRIDAD (que viene del publicador y
//! no fue alterado), NO inocuidad del codigo del mod — el amigo igual confia en
//! que el publicador no metio un .dll malicioso (es para amigos, no enterprise).
//! La clave PRIVADA jamas toca al cliente; se guarda fuera del repo.

use anyhow::{Context, Result};
use minisign_verify::{PublicKey, Signature};

/// Clave publica minisign del publicador, empotrada en el binario. FASE 2:
/// reemplazar por la real (`minisign -G` genera el par). Vacia = firma
/// deshabilitada (SOLO desarrollo).
pub const PUBLISHER_PUBKEY: &str = "";

/// Verifica `signature` (formato minisign) sobre `manifest_bytes` con `pubkey_b64`.
pub fn verify(pubkey_b64: &str, manifest_bytes: &[u8], signature: &str) -> Result<()> {
    let pk = PublicKey::from_base64(pubkey_b64).context("clave publica minisign invalida")?;
    let sig = Signature::decode(signature).context("firma minisign invalida")?;
    pk.verify(manifest_bytes, &sig, false)
        .map_err(|e| anyhow::anyhow!("la firma del manifiesto NO valida: {e}"))
}

/// Verifica con la clave empotrada. Si `PUBLISHER_PUBKEY` esta vacia, advierte y
/// NO falla (modo dev). En release la clave DEBE estar seteada y la firma presente.
pub fn verify_with_embedded(manifest_bytes: &[u8], signature: Option<&str>) -> Result<()> {
    if PUBLISHER_PUBKEY.is_empty() {
        eprintln!("[seguridad] PUBLISHER_PUBKEY vacia: firma NO verificada (modo dev).");
        return Ok(());
    }
    let sig = signature.context("el set no trae firma y la verificacion es obligatoria")?;
    verify(PUBLISHER_PUBKEY, manifest_bytes, sig)
}
