//! Deteccion del install de Slay the Spire 2 en Windows.
//!
//! Cascada: Steam (steamlocate, AppID 2868840) -> barrido de rutas comunes ->
//! (si nada) dialogo manual `pick_folder_dialog`, pensado para las copias PIRATA
//! que no estan en Steam. SIEMPRE se valida la carpeta por heuristica
//! (`is_valid_install`), nunca por el nombre.

use crate::STS2_STEAM_APPID;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Ejecutable y carpeta de datos que delatan un install valido de StS2.
const EXE: &str = "SlayTheSpire2.exe";
const DATA_DIR: &str = "data_sts2_windows_x86_64";

#[derive(Debug, Clone)]
pub struct Install {
    /// Raiz del juego (contiene SlayTheSpire2.exe).
    pub root: PathBuf,
    /// `<root>/mods` (donde van los <Id>/).
    pub mods_dir: PathBuf,
    /// Version del juego segun release_info.json (p.ej. "v0.103.3"), si se pudo leer.
    pub version: Option<String>,
    /// Como se hallo.
    pub source: Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Steam,
    Heuristic,
    Manual,
    Cached,
}

/// Cascada de deteccion automatica. `None` => la UI debe ofrecer `pick_folder_dialog`.
pub fn detect() -> Option<Install> {
    detect_steam().or_else(detect_common_paths)
}

/// Construye un Install a partir de una raiz conocida (p.ej. cacheada en config),
/// re-validandola. `None` si la carpeta ya no es un install valido.
pub fn from_root(root: &Path) -> Option<Install> {
    finalize(root.to_path_buf(), Source::Cached)
}

fn detect_steam() -> Option<Install> {
    let dir = steamlocate::SteamDir::locate().ok()?;
    // find_app: Ok(Some((App, Library))) si esta instalado en alguna biblioteca.
    let (app, library) = dir.find_app(STS2_STEAM_APPID).ok()??;
    let root = library.resolve_app_dir(&app);
    finalize(root, Source::Steam)
}

fn detect_common_paths() -> Option<Install> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for drive in ['C', 'D', 'E', 'F', 'G'] {
        let base = format!("{drive}:\\");
        for rel in [
            "Program Files (x86)\\Steam\\steamapps\\common\\Slay the Spire 2",
            "SteamLibrary\\steamapps\\common\\Slay the Spire 2",
            "Games\\Slay the Spire 2",
            "Slay the Spire 2",
        ] {
            candidates.push(Path::new(&base).join(rel));
        }
    }
    candidates
        .into_iter()
        .find_map(|p| finalize(p, Source::Heuristic))
}

/// Dialogo nativo de seleccion de carpeta (Win32, sin GTK). Valida lo elegido.
pub fn pick_folder_dialog() -> Option<Install> {
    let picked = rfd::FileDialog::new()
        .set_title("Elegi la carpeta de Slay the Spire 2")
        .pick_folder()?;
    finalize(picked, Source::Manual)
}

fn finalize(root: PathBuf, source: Source) -> Option<Install> {
    if !is_valid_install(&root) {
        return None;
    }
    let mods_dir = root.join("mods");
    let version = read_version(&root);
    Some(Install {
        root,
        mods_dir,
        version,
        source,
    })
}

/// Regla minima de validacion: existe el exe Y la carpeta de datos.
pub fn is_valid_install(root: &Path) -> bool {
    root.join(EXE).is_file() && root.join(DATA_DIR).is_dir()
}

/// Lee `release_info.json` -> campo "version".
pub fn read_version(root: &Path) -> Option<String> {
    let txt = std::fs::read_to_string(root.join("release_info.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
    v.get("version")?.as_str().map(str::to_string)
}

/// Crea `<root>/mods` si falta (las copias pirata recien instaladas pueden no tenerla).
pub fn ensure_mods_dir(install: &Install) -> Result<()> {
    if !install.mods_dir.is_dir() {
        std::fs::create_dir_all(&install.mods_dir)?;
    }
    Ok(())
}

/// True si el proceso del juego esta corriendo (sus .dll/.pck quedan lockeados en
/// Windows; no se debe escribir `mods/` hasta cerrarlo).
pub fn is_game_running() -> bool {
    use sysinfo::System;
    let sys = System::new_all();
    let full = EXE.to_ascii_lowercase();
    sys.processes().values().any(|p| {
        name_is_game(&p.name().to_string_lossy())
            // Fallback: el basename del path del ejecutable (por si el nombre vino raro).
            || p.exe()
                .and_then(|e| e.file_name())
                .map(|f| f.to_string_lossy().to_ascii_lowercase() == full)
                .unwrap_or(false)
    })
}

/// True si `proc_name` corresponde al exe del juego. Tolerante a: mayusculas/minusculas, a que
/// falte el sufijo `.exe`, y a truncamiento del nombre (sysinfo puede cortarlo en algunas
/// plataformas). Un falso NEGATIVO seria grave: dejaria mutar `mods/` con el juego abierto.
fn name_is_game(proc_name: &str) -> bool {
    let full = EXE.to_ascii_lowercase();
    let stem = full.strip_suffix(".exe").unwrap_or(&full);
    let name = proc_name.to_ascii_lowercase();
    let name_stem = name.strip_suffix(".exe").unwrap_or(&name);
    name_stem == stem
        // truncado: "slaythespir..." con >=8 chars que prefijan el nombre real.
        || (name_stem.len() >= 8 && stem.starts_with(name_stem))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_game_es_tolerante() {
        assert!(name_is_game("SlayTheSpire2.exe")); // exacto
        assert!(name_is_game("slaythespire2.exe")); // minuscula
        assert!(name_is_game("SLAYTHESPIRE2.EXE")); // mayuscula
        assert!(name_is_game("SlayTheSpire2")); // sin .exe
        assert!(name_is_game("slaythespire")); // truncado (>=8, prefijo)
        // NO debe matchear:
        assert!(!name_is_game("notepad.exe"));
        assert!(!name_is_game("slay")); // prefijo corto (<8) -> evita falsos positivos
        assert!(!name_is_game("SlayTheSpire3.exe"));
        assert!(!name_is_game(""));
    }
}
