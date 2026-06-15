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
/// cierra path-traversal al construir rutas destino. Comparte el predicado con el manifest.
fn safe_id(id: &str) -> Result<&str> {
    if !crate::manifest::is_simple_segment(id) {
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
    // Extraccion MANUAL anti zip-slip: el install local desde `.zip` NO pasa por
    // `validate_paths`, asi que se cierra aca. `enclosed_name()` descarta nombres con `..` o
    // absolutos que escaparian de `tmp`; el chequeo por componentes es la red secundaria.
    for i in 0..archive.len() {
        let mut zf = archive.by_index(i).context("entrada de zip invalida")?;
        let rel = match zf.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => bail!("entrada de zip insegura (zip-slip): {:?}", zf.name()),
        };
        // Red secundaria EFECTIVA: reconfirmar por componentes que `rel` no tenga `..`/raiz/
        // prefijo (`Path::starts_with(tmp)` NO sirve: es lexico y no normaliza `..`).
        use std::path::Component;
        if rel
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            bail!("entrada de zip insegura: {:?}", zf.name());
        }
        let out = tmp.join(&rel);
        if zf.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut outf = std::fs::File::create(&out)
                .with_context(|| format!("creando {}", out.display()))?;
            std::io::copy(&mut zf, &mut outf)
                .with_context(|| format!("extrayendo {}", out.display()))?;
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{Install, Source};

    #[test]
    fn safe_id_rechaza_traversal_y_acepta_normal() {
        for bad in [
            "", "..", ".", "a/b", "a\\b", "C:evil", "../x", "x\\..", "/abs",
        ] {
            assert!(safe_id(bad).is_err(), "deberia rechazar {bad:?}");
        }
        assert_eq!(safe_id("BaseLib").unwrap(), "BaseLib");
        assert_eq!(safe_id("FGO_Core-1.2").unwrap(), "FGO_Core-1.2");
    }

    fn make_mod(dir: &Path, id: &str) {
        std::fs::create_dir_all(dir.join(id)).unwrap();
        std::fs::write(
            dir.join(id).join(format!("{id}.json")),
            format!(r#"{{"id":"{id}"}}"#),
        )
        .unwrap();
    }

    fn temp_install(name: &str) -> Install {
        let base = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&base);
        let mods_dir = base.join("mods");
        std::fs::create_dir_all(&mods_dir).unwrap();
        Install {
            root: base,
            mods_dir,
            version: None,
            source: Source::Manual,
        }
    }

    #[test]
    fn enable_disable_round_trip() {
        // enable/disable exigen el juego cerrado (mueven carpetas).
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let install = temp_install("sts2_modsync_manager_endis");
        make_mod(&install.mods_dir, "Mod");

        disable(&install, "Mod").unwrap();
        assert!(!install.mods_dir.join("Mod").exists());
        assert!(disabled_dir(&install).join("Mod").is_dir());

        enable(&install, "Mod").unwrap();
        assert!(install.mods_dir.join("Mod").is_dir());
        assert!(!disabled_dir(&install).join("Mod").exists());

        // disable de algo que no esta habilitado -> error.
        assert!(disable(&install, "NoExiste").is_err());
        let _ = std::fs::remove_dir_all(&install.root);
    }

    #[test]
    fn install_from_dir_copia_y_respeta_no_overwrite() {
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let install = temp_install("sts2_modsync_manager_install");
        // carpeta fuente con un mod valido, FUERA de mods/.
        let src_parent = install.root.join("incoming");
        make_mod(&src_parent, "Mod"); // crea incoming/Mod/Mod.json
        let src = src_parent.join("Mod");

        let id = install_from_dir(&install, &src, false).unwrap();
        assert_eq!(id, "Mod");
        assert!(install.mods_dir.join("Mod").join("Mod.json").is_file());

        // segunda vez sin overwrite -> error (ya existe).
        assert!(install_from_dir(&install, &src, false).is_err());
        let _ = std::fs::remove_dir_all(&install.root);
    }

    #[test]
    fn extract_zip_rechaza_zip_slip() {
        use std::io::Write;
        let dir = std::env::temp_dir().join("sts2_modsync_zipslip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("evil.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            // entrada que intenta escapar del destino (zip-slip).
            zw.start_file("../escape.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(b"pwned").unwrap();
            zw.finish().unwrap();
        }
        // el destino REAL del escape: la extraccion va a temp_dir()/sts2_install_<n>, asi que
        // "../escape.txt" caeria en temp_dir()/escape.txt (NO en `dir`, que era un assert muerto).
        let real_escape = std::env::temp_dir().join("escape.txt");
        let _ = std::fs::remove_file(&real_escape);
        assert!(
            extract_zip_to_temp(&zip_path).is_err(),
            "un zip con ../ debe ser rechazado"
        );
        assert!(
            !real_escape.exists(),
            "el zip-slip NO debe escribir fuera del temp"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_from_zip_extrae_e_instala() {
        use std::io::Write;
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let install = temp_install("sts2_modsync_manager_zipinstall");
        // .zip con un mod valido adentro (Mod/Mod.json), anidado en una carpeta.
        let zip_path = install.root.join("mod.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            zw.start_file("Mod/Mod.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(br#"{"id":"Mod"}"#).unwrap();
            zw.finish().unwrap();
        }
        let id = install_from_zip(&install, &zip_path, false).unwrap();
        assert_eq!(id, "Mod");
        assert!(install.mods_dir.join("Mod").join("Mod.json").is_file());
        let _ = std::fs::remove_dir_all(&install.root);
    }

    #[test]
    fn uninstall_saca_el_mod_y_falla_si_no_existe() {
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let install = temp_install("sts2_modsync_manager_uninstall");
        make_mod(&install.mods_dir, "Mod");
        assert!(install.mods_dir.join("Mod").is_dir());

        assert!(uninstall(&install, "NoExiste").is_err()); // mod inexistente -> error

        // uninstall manda a la papelera del SO (puede no estar disponible en CI headless).
        match uninstall(&install, "Mod") {
            Ok(()) => assert!(mod_dir(&install, "Mod").is_none(), "deberia salir de mods/"),
            Err(_) => eprintln!("(skip: papelera no disponible en este entorno)"),
        }
        let _ = std::fs::remove_dir_all(&install.root);
    }
}
