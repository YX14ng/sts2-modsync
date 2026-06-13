//! Mod manager — operaciones que MUTAN el install: habilitar/deshabilitar (mover la
//! carpeta entre `mods/` y `mods_disabled/`), instalar (copiar carpeta o extraer .zip a
//! `mods/`) y desinstalar (a la papelera del SO). Todas exigen el juego CERRADO (lock de
//! .dll/.pck en Windows) y validan que el id no escape de `mods/`. NO toca `setting.save`
//! (el orden de carga lo impone ModListSorter); ver HANDOFF.

use crate::detect::Install;
use crate::modlist::{self, disabled_dir};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// Aborta si el juego corre (sus .dll/.pck quedan lockeados; mover/borrar fallaria o
/// dejaria el set inconsistente).
fn ensure_game_closed() -> Result<()> {
    if crate::detect::is_game_running() {
        bail!("El juego esta ABIERTO — cerralo antes de tocar los mods (lock de .dll/.pck).");
    }
    Ok(())
}

/// Valida que `id` sea un nombre de carpeta simple (sin separadores, `..` ni absolutas):
/// cierra path-traversal al construir rutas destino (mismo criterio que `validate_paths`).
fn safe_id(id: &str) -> Result<&str> {
    if id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains(':')
        || id == ".."
        || id == "."
    {
        bail!("id de mod invalido: {id:?}");
    }
    Ok(id)
}

/// Carpeta actual de un mod instalado (en `mods/` o `mods_disabled/`), si existe.
pub fn mod_dir(install: &Install, id: &str) -> Option<PathBuf> {
    let id = safe_id(id).ok()?;
    let enabled = install.mods_dir.join(id);
    if enabled.is_dir() {
        return Some(enabled);
    }
    let disabled = disabled_dir(install).join(id);
    disabled.is_dir().then_some(disabled)
}

/// Deshabilita un mod: mueve `mods/<id>` -> `mods_disabled/<id>`.
pub fn disable(install: &Install, id: &str) -> Result<()> {
    ensure_game_closed()?;
    let id = safe_id(id)?;
    let src = install.mods_dir.join(id);
    if !src.is_dir() {
        bail!("el mod {id:?} no esta habilitado");
    }
    let dst_dir = disabled_dir(install);
    std::fs::create_dir_all(&dst_dir)?;
    move_dir(&src, &dst_dir.join(id))
}

/// Habilita un mod: mueve `mods_disabled/<id>` -> `mods/<id>`.
pub fn enable(install: &Install, id: &str) -> Result<()> {
    ensure_game_closed()?;
    let id = safe_id(id)?;
    let src = disabled_dir(install).join(id);
    if !src.is_dir() {
        bail!("el mod {id:?} no esta deshabilitado");
    }
    std::fs::create_dir_all(&install.mods_dir)?;
    move_dir(&src, &install.mods_dir.join(id))
}

/// Desinstala un mod (habilitado o no) mandandolo a la papelera del SO (reversible).
pub fn uninstall(install: &Install, id: &str) -> Result<()> {
    ensure_game_closed()?;
    let id = safe_id(id)?;
    let dir = mod_dir(install, id).with_context(|| format!("el mod {id:?} no esta instalado"))?;
    trash::delete(&dir).with_context(|| format!("no se pudo mandar {id:?} a la papelera"))?;
    Ok(())
}

/// Instala un mod desde una carpeta suelta (que contiene su `<id>.json`). Devuelve el id.
/// Si ya hay un mod con ese id y `!overwrite`, falla.
pub fn install_from_dir(install: &Install, src: &Path, overwrite: bool) -> Result<String> {
    ensure_game_closed()?;
    let manifest = modlist::read_manifest(src)
        .context("la carpeta no contiene un <id>.json valido de un mod")?;
    let id = safe_id(&manifest.id)?.to_string();
    let dst = install.mods_dir.join(&id);
    prepare_dst(install, &id, overwrite)?;
    copy_dir(src, &dst)?;
    Ok(id)
}

/// Instala un mod desde un `.zip` (busca dentro la carpeta con el `<id>.json`).
pub fn install_from_zip(install: &Install, zip_path: &Path, overwrite: bool) -> Result<String> {
    ensure_game_closed()?;
    let tmp = extract_zip_to_temp(zip_path)?;
    let res = (|| {
        let mod_root = find_mod_root(&tmp).context("el .zip no contiene un mod con <id>.json")?;
        install_from_dir(install, &mod_root, overwrite)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    res
}

/// Abre una carpeta en el explorador de Windows.
pub fn open_folder(path: &Path) -> Result<()> {
    std::process::Command::new("explorer")
        .arg(path)
        .spawn()
        .with_context(|| format!("no se pudo abrir {}", path.display()))?;
    Ok(())
}

// --- helpers de filesystem ---------------------------------------------------

/// Si ya hay un mod con `id` (habilitado o no): falla salvo `overwrite` (en cuyo caso lo
/// manda a la papelera). Asegura que exista `mods/`.
fn prepare_dst(install: &Install, id: &str, overwrite: bool) -> Result<()> {
    if let Some(existing) = mod_dir(install, id) {
        if !overwrite {
            bail!("ya hay un mod {id:?} instalado (usa 'reemplazar' para sobreescribir)");
        }
        trash::delete(&existing).with_context(|| format!("reemplazando {id:?}"))?;
    }
    std::fs::create_dir_all(&install.mods_dir)?;
    Ok(())
}

/// Mueve un directorio: `rename` (instantaneo en el mismo volumen) con fallback a
/// copiar+borrar (cross-device). Falla si el destino ya existe.
fn move_dir(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        bail!("el destino ya existe: {}", dst.display());
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_dir(src, dst)?;
    std::fs::remove_dir_all(src)
        .with_context(|| format!("borrando el original {}", src.display()))?;
    Ok(())
}

/// Copia recursivamente `src` dentro de `dst`.
fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in walkdir::WalkDir::new(src).into_iter().flatten() {
        let rel = match entry.path().strip_prefix(src) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else {
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::copy(entry.path(), &target)
                .with_context(|| format!("copiando {}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn extract_zip_to_temp(zip_path: &Path) -> Result<PathBuf> {
    let file = std::fs::File::open(zip_path)
        .with_context(|| format!("abriendo {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("zip invalido")?;
    let tmp = unique_temp_dir("sts2_install");
    std::fs::create_dir_all(&tmp)?;
    archive.extract(&tmp).context("extrayendo el zip")?;
    Ok(tmp)
}

/// Busca la carpeta del mod dentro de `dir` (el zip puede traerlo en la raiz o anidado).
fn find_mod_root(dir: &Path) -> Option<PathBuf> {
    if modlist::read_manifest(dir).is_some() {
        return Some(dir.to_path_buf());
    }
    for entry in walkdir::WalkDir::new(dir)
        .min_depth(1)
        .max_depth(3)
        .into_iter()
        .flatten()
    {
        if entry.file_type().is_dir() && modlist::read_manifest(entry.path()).is_some() {
            return Some(entry.path().to_path_buf());
        }
    }
    None
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("{prefix}_{nanos}"))
}
