//! Lanzar Slay the Spire 2. El menu principal del juego ofrece "Load with Mods", asi que
//! alcanza con abrir el exe.
//!
//! OJO build de STEAM: el juego llama a `SteamAPI_Init`, que falla con
//! `k_ESteamAPIInitResult_FailedGeneric: No appID found` si el `.exe` se corre DIRECTO (no desde el
//! cliente de Steam). El arreglo estandar es dejar un `steam_appid.txt` con el appID en la carpeta del
//! juego: ahi SteamAPI inicializa contra el Steam que ya esta corriendo. Asi seguimos abriendo el exe
//! igual (mismo flujo de mods) pero sin el error. Las copias pirata (sin la dll de Steamworks) no lo
//! necesitan y no se tocan.

use crate::STS2_STEAM_APPID;
use crate::detect::Install;
use anyhow::{Context, Result};
use std::process::Command;

/// Lanza el juego desde su carpeta de install.
pub fn launch(install: &Install) -> Result<()> {
    ensure_steam_appid(&install.root); // build de Steam: evita "No appID found" al correr el exe directo
    let exe = install.root.join("SlayTheSpire2.exe");
    Command::new(&exe)
        .current_dir(&install.root)
        .spawn()
        .with_context(|| format!("no se pudo lanzar {}", exe.display()))?;
    Ok(())
}

/// `true` si la carpeta tiene la dll de Steamworks (build de Steam, o copia con un emulador tipo
/// Goldberg — que tambien lee `steam_appid.txt`). Las copias pirata "limpias" no la tienen.
fn has_steamworks(root: &std::path::Path) -> bool {
    root.join("steam_api64.dll").is_file() || root.join("steam_api.dll").is_file()
}

/// Si hay evidencia de Steam y `steam_appid.txt` no tiene ya el appID correcto, lo escribe. Best-effort:
/// un fallo de escritura NO impide lanzar (el juego solo mostrara el error de Steam, como antes).
fn ensure_steam_appid(root: &std::path::Path) {
    if !has_steamworks(root) {
        return; // sin Steamworks (pirata limpia): el juego no usa SteamAPI, no hace falta el txt
    }
    let appid = STS2_STEAM_APPID.to_string();
    let txt = root.join("steam_appid.txt");
    // No reescribir si ya esta correcto (no tocar el archivo en cada lanzamiento).
    if std::fs::read_to_string(&txt)
        .map(|s| s.trim() == appid)
        .unwrap_or(false)
    {
        return;
    }
    let _ = std::fs::write(&txt, &appid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_steam_appid_solo_con_steamworks() {
        let dir = std::env::temp_dir().join(format!("sts2_launch_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let txt = dir.join("steam_appid.txt");

        // Sin la dll de Steamworks (pirata limpia): NO se escribe steam_appid.txt.
        ensure_steam_appid(&dir);
        assert!(!txt.exists(), "sin Steamworks no deberia escribir el txt");

        // Con la dll (build de Steam): se escribe con el appID correcto.
        std::fs::write(dir.join("steam_api64.dll"), b"x").unwrap();
        ensure_steam_appid(&dir);
        assert_eq!(
            std::fs::read_to_string(&txt).unwrap().trim(),
            STS2_STEAM_APPID.to_string()
        );

        // Idempotente: si ya esta correcto, no falla ni cambia.
        ensure_steam_appid(&dir);
        assert_eq!(
            std::fs::read_to_string(&txt).unwrap().trim(),
            STS2_STEAM_APPID.to_string()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
