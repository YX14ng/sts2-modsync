//! Lanzar Slay the Spire 2. El menu principal del juego ofrece "Load with Mods", asi que
//! alcanza con abrir el exe (los mods se leen de `mods/` sea cual sea el lanzador).
//!
//! DOS modos para el build de STEAM (el juego llama a `SteamAPI_Init`, que falla con
//! `k_ESteamAPIInitResult_FailedGeneric: No appID found` si el `.exe` se corre DIRECTO):
//!  - **por Steam** (`config.launch_via_steam`, default): `steam://rungameid/<appid>` — Steam lanza
//!    el juego con integracion COMPLETA (overlay, horas, invitaciones). Es el modo recomendado.
//!  - **directo**: se abre el exe, dejando antes un `steam_appid.txt` con el appID para que SteamAPI
//!    inicialice contra el Steam que ya corre (sin overlay, pero anda).
//!
//! Las copias PIRATA (sin la dll de Steamworks) no usan SteamAPI: siempre van directo y no se tocan.

use crate::STS2_STEAM_APPID;
use crate::detect::Install;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

const EXE: &str = "SlayTheSpire2.exe";

/// Lanza el juego. `via_steam` (de `config.launch_via_steam`): si es un build de Steam, lanzarlo POR
/// Steam (overlay) en vez de abrir el exe directo. No aplica a copias pirata (siempre directo).
pub fn launch(install: &Install, via_steam: bool) -> Result<()> {
    if via_steam && is_steam_build(&install.root) {
        return launch_via_steam(STS2_STEAM_APPID);
    }
    ensure_steam_appid(&install.root); // build de Steam: evita "No appID found" al correr el exe directo
    let exe = install.root.join(EXE);
    Command::new(&exe)
        .current_dir(&install.root)
        .spawn()
        .with_context(|| format!("no se pudo lanzar {}", exe.display()))?;
    Ok(())
}

/// Pide a Steam que lance el juego (`steam://rungameid/<appid>`). Lo abre con el handler del
/// protocolo (`explorer` resuelve `steam://`); Steam ya esta corriendo si el juego es de Steam.
fn launch_via_steam(appid: u32) -> Result<()> {
    let url = format!("steam://rungameid/{appid}");
    // `explorer <url>` invoca el handler registrado del protocolo (Steam). No bloquea.
    Command::new("explorer")
        .arg(&url)
        .spawn()
        .with_context(|| format!("no se pudo abrir {url} (¿esta Steam instalado?)"))?;
    Ok(())
}

/// `true` si la carpeta tiene la dll de Steamworks (build de Steam, o copia con un emulador tipo
/// Goldberg — que tambien lee `steam_appid.txt`). Las copias pirata "limpias" no la tienen.
pub fn is_steam_build(root: &Path) -> bool {
    root.join("steam_api64.dll").is_file() || root.join("steam_api.dll").is_file()
}

/// Si hay evidencia de Steam y `steam_appid.txt` no tiene ya el appID correcto, lo escribe. Best-effort:
/// un fallo de escritura NO impide lanzar (el juego solo mostrara el error de Steam, como antes).
fn ensure_steam_appid(root: &Path) {
    if !is_steam_build(root) {
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
        assert!(!is_steam_build(&dir));
        ensure_steam_appid(&dir);
        assert!(!txt.exists(), "sin Steamworks no deberia escribir el txt");

        // Con la dll (build de Steam): se escribe con el appID correcto.
        std::fs::write(dir.join("steam_api64.dll"), b"x").unwrap();
        assert!(is_steam_build(&dir));
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
