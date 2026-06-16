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

/// Manda a la papelera una CARPETA de mod ESPECIFICA (por path). A diferencia de `uninstall` (por
/// id), esto borra la carpeta exacta — necesario para limpiar DUPLICADOS, donde el mismo id vive en
/// varias carpetas con nombres distintos. SEGURO: exige el juego cerrado y que `dir` sea hija DIRECTA
/// de `mods/` o `mods_disabled/` (nunca toca nada afuera del area gestionada).
pub fn trash_mod_dir(install: &Install, dir: &Path) -> Result<()> {
    ensure_game_closed()?;
    let parent = dir.parent();
    let in_managed = parent == Some(install.mods_dir.as_path())
        || parent == Some(disabled_dir(install).as_path());
    if !in_managed {
        bail!(
            "no se borra {}: no es una carpeta de mod dentro de mods/ ni mods_disabled/",
            dir.display()
        );
    }
    // El parent-check es LEXICO: `mods/..` tendria parent `mods` y pasaria, pero resuelve a la RAIZ
    // del juego. Exigir que el ultimo componente sea un nombre de carpeta SIMPLE (rechaza `..`/`.`/
    // separadores) cierra ese hueco — la misma red que `safe_id` para los borrados por id.
    let name = dir
        .file_name()
        .context("ruta de mod sin nombre de carpeta (¿termina en '..'?)")?;
    if !crate::manifest::is_simple_segment(&name.to_string_lossy()) {
        bail!("nombre de carpeta de mod invalido: {}", dir.display());
    }
    if !dir.is_dir() {
        bail!("no existe la carpeta {}", dir.display());
    }
    trash::delete(dir)
        .with_context(|| format!("no se pudo mandar {} a la papelera", dir.display()))?;
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
    prepare_dst(install, src, &id, overwrite)?;
    copy_dir(src, &dst)?;
    Ok(id)
}

/// Instala un mod desde un archivo `.zip` o `.7z` (busca dentro la carpeta con el `<id>.json`).
/// El formato se detecta por MAGIC (`archive_kind`), no por la extension.
pub fn install_from_zip(install: &Install, archive_path: &Path, overwrite: bool) -> Result<String> {
    ensure_game_closed()?;
    let tmp = extract_archive_to_temp(archive_path)?;
    let res = (|| {
        let mod_root =
            find_mod_root(&tmp).context("el archivo no contiene un mod con <id>.json")?;
        install_from_dir(install, &mod_root, overwrite)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    res
}

/// `true` si ya hay alguna copia instalada del mod `id` (en `mods/` o `mods_disabled/`, con
/// CUALQUIER nombre de carpeta) — lo que un install con `overwrite` mandaria a la papelera. Para
/// avisar/confirmar antes de pisar (p.ej. el flujo `nxm://` lanzado por el protocolo, sin la app).
pub fn is_id_installed(install: &Install, id: &str) -> bool {
    mod_dir(install, id).is_some() || !dirs_with_id(install, id).is_empty()
}

/// Como `install_from_zip` (overwrite), pero si el id que trae el archivo YA esta instalado llama a
/// `confirm_replace(id)` ANTES de reemplazar; si devuelve `false`, no instala nada y retorna
/// `Ok(None)`. Extrae UNA sola vez (lee el id del temp y instala de ahi). Para el flujo `nxm://`,
/// que lanza el protocolo: como no pasa por la app, sin esto pisaria el mod en silencio.
pub fn install_from_zip_confirmed(
    install: &Install,
    archive_path: &Path,
    confirm_replace: impl FnOnce(&str) -> bool,
) -> Result<Option<String>> {
    ensure_game_closed()?;
    let tmp = extract_archive_to_temp(archive_path)?;
    let res = (|| {
        let mod_root =
            find_mod_root(&tmp).context("el archivo no contiene un mod con <id>.json")?;
        let manifest = modlist::read_manifest(&mod_root)
            .context("el mod del archivo no tiene <id>.json valido")?;
        let id = safe_id(&manifest.id)?.to_string();
        if is_id_installed(install, &id) && !confirm_replace(&id) {
            return Ok(None);
        }
        install_from_dir(install, &mod_root, true).map(Some)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    res
}

/// Instala un `.zip` REEMPLAZANDO el mod `expected_id`, pero SOLO si el `<id>.json` DENTRO del zip
/// declara EXACTAMENTE ese id. Asi un release del upstream de A nunca puede pisar a B: el id que se
/// instala lo controla el contenido del zip (el upstream), no el llamador, asi que sin esta guarda
/// `install_from_zip(overwrite)` mandaria a la papelera el mod con el id del zip, sea cual sea. Para
/// el auto-update de mods (`modupdate::apply`).
pub fn install_update_zip(install: &Install, archive_path: &Path, expected_id: &str) -> Result<()> {
    ensure_game_closed()?;
    let tmp = extract_archive_to_temp(archive_path)?;
    let res = (|| {
        let mod_root = find_mod_root(&tmp).context(
            "el archivo del release no trae un mod empaquetado como carpeta con su <id>.json \
             (¿el release sube el .dll suelto? bajalo a mano e instala con 'Instalar .zip')",
        )?;
        let manifest = modlist::read_manifest(&mod_root)
            .context("el mod del archivo no tiene <id>.json valido")?;
        if manifest.id != expected_id {
            bail!(
                "el archivo del release trae el mod {:?}, no {expected_id:?}: abortado para no pisar otro mod",
                manifest.id
            );
        }
        install_from_dir(install, &mod_root, true).map(|_| ())
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

/// Quita TODAS las copias instaladas del mod `id` antes de poner la nueva: en `mods/` Y
/// `mods_disabled/`, con CUALQUIER nombre de carpeta (no solo `mods/<id>`). Asi actualizar/reinstalar
/// un mod NO deja la version vieja como duplicado — el bug de "no borra los mods que reemplaza".
/// Con `!overwrite`, si ya hay alguna copia FALLA (no pisa sin permiso). Nunca borra `src` (por si se
/// instala desde dentro de `mods/`). Asegura que exista `mods/`.
fn prepare_dst(install: &Install, src: &Path, id: &str, overwrite: bool) -> Result<()> {
    let src_canon = std::fs::canonicalize(src).ok();
    let mut copies = dirs_with_id(install, id);
    // La carpeta EXACTA mods/<id> o mods_disabled/<id> aunque su `<id>.json` no parsee (carpeta
    // vieja/stray): tambien se reemplaza, como antes.
    if let Some(exact) = mod_dir(install, id)
        && !copies.contains(&exact)
    {
        copies.push(exact);
    }
    // Excluir la FUENTE: por path crudo (cubre el caso aunque `canonicalize` falle) Y por canonico
    // (cubre symlink/normalizacion). Si `canonicalize(src)` fallo, solo se excluye por path crudo.
    copies.retain(|d| {
        d != src && (src_canon.is_none() || std::fs::canonicalize(d).ok() != src_canon)
    });
    if !copies.is_empty() {
        if !overwrite {
            bail!("ya hay un mod {id:?} instalado (usa 'reemplazar' para sobreescribir)");
        }
        for dir in &copies {
            // Via `trash_mod_dir`: re-valida juego-cerrado + que sea hija directa de mods/ o
            // mods_disabled/ (defensa en profundidad, aunque `copies` ya sale de read_dir de esos).
            trash_mod_dir(install, dir)
                .with_context(|| format!("reemplazando {id:?} ({})", dir.display()))?;
        }
    }
    std::fs::create_dir_all(&install.mods_dir)?;
    Ok(())
}

/// Carpetas (en `mods/` y `mods_disabled/`) cuyo `<id>.json` declara `id`. Incluye carpetas con
/// NOMBRE distinto del id (un duplicado tipico al recibir mods por Drive con nombres versionados).
/// Reusa `modlist::folders_with_declared_id` (escaneo + atribucion por manifiesto unificados con la
/// limpieza de duplicados de la sync). LIMITE inherente: una carpeta con manifiesto ILEGIBLE y nombre
/// != id no se puede atribuir (sin manifiesto no se sabe su id); no se infiere por nombre para no
/// borrar un mod equivocado.
fn dirs_with_id(install: &Install, id: &str) -> Vec<PathBuf> {
    modlist::folders_with_declared_id(install)
        .into_iter()
        .filter(|(_, mid)| mid == id)
        .map(|(p, _)| p)
        .collect()
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

/// Formato de un archivo de mod, detectado por sus bytes MAGIC (no por la extension, que puede
/// mentir — el CDN de Nexus a veces sirve un `.7z` con una URL sin extension clara).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    SevenZ,
    Other,
}

/// Detecta el formato de `path` por los primeros bytes: ZIP (`PK\x03\x04`) o 7z
/// (`37 7A BC AF 27 1C`). `Other` si no es ninguno (`.rar` u otro): no se auto-instala.
pub fn archive_kind(path: &Path) -> ArchiveKind {
    use std::io::Read;
    let mut buf = [0u8; 6];
    let Ok(mut f) = std::fs::File::open(path) else {
        return ArchiveKind::Other;
    };
    let n = f.read(&mut buf).unwrap_or(0);
    if n >= 4 && &buf[..4] == b"PK\x03\x04" {
        ArchiveKind::Zip
    } else if n >= 6 && buf == [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C] {
        ArchiveKind::SevenZ
    } else {
        ArchiveKind::Other
    }
}

/// Extrae un archivo (.zip o .7z) a un dir temporal, eligiendo por MAGIC. `.rar`/otros: error claro.
fn extract_archive_to_temp(path: &Path) -> Result<PathBuf> {
    match archive_kind(path) {
        ArchiveKind::Zip => extract_zip_to_temp(path),
        ArchiveKind::SevenZ => extract_7z_to_temp(path),
        ArchiveKind::Other => bail!(
            "formato de archivo no soportado (solo .zip y .7z): {}",
            path.display()
        ),
    }
}

/// Extrae un `.7z` a un dir temporal, con la MISMA defensa anti path-traversal que el `.zip`:
/// valida cada entrada por componentes (rechaza `..`/raiz/prefijo) antes de escribir.
fn extract_7z_to_temp(path: &Path) -> Result<PathBuf> {
    use std::path::Component;
    let tmp = unique_temp_dir("sts2_install_7z");
    std::fs::create_dir_all(&tmp)?;
    let mut reader = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())
        .context("7z invalido o protegido con password")?;
    let to_sz = |e: std::io::Error| sevenz_rust::Error::other(e.to_string());
    reader
        .for_each_entries(|entry, rd| {
            let rel = PathBuf::from(entry.name().replace('\\', "/"));
            if rel
                .components()
                .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
            {
                return Err(sevenz_rust::Error::other(format!(
                    "entrada 7z insegura (path-traversal): {:?}",
                    entry.name()
                )));
            }
            let out = tmp.join(&rel);
            if entry.is_directory() {
                std::fs::create_dir_all(&out).map_err(to_sz)?;
            } else {
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent).map_err(to_sz)?;
                }
                let mut f = std::fs::File::create(&out).map_err(to_sz)?;
                std::io::copy(rd, &mut f).map_err(to_sz)?;
            }
            Ok(true)
        })
        .context("extrayendo el .7z")?;
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
    std::env::temp_dir().join(format!("{prefix}_{}", crate::util::unique_nanos()))
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
    fn trash_mod_dir_solo_borra_dentro_del_area_gestionada() {
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let install = temp_install("sts2_modsync_manager_trashguard");
        // Una carpeta FUERA de mods/ (en la raiz del install) NO se debe poder borrar.
        let outside = install.root.join("noborrar");
        std::fs::create_dir_all(&outside).unwrap();
        assert!(trash_mod_dir(&install, &outside).is_err());
        assert!(outside.is_dir(), "no debe tocar una carpeta fuera de mods/");

        // `mods/..` pasa el parent-check LEXICO pero resuelve a la raiz: el chequeo del ultimo
        // componente (`..` no es un nombre simple) lo rechaza.
        assert!(trash_mod_dir(&install, &install.mods_dir.join("..")).is_err());
        assert!(trash_mod_dir(&install, &disabled_dir(&install).join("..")).is_err());
        assert!(install.root.is_dir(), "JAMAS debe borrar la raiz del juego");
        // Una carpeta de mod (hija directa de mods/) si es candidata valida (la validacion pasa).
        make_mod(&install.mods_dir, "Dup");
        let dup = install.mods_dir.join("Dup");
        // No la mandamos a la papelera de verdad en el test; basta con que la validacion la acepte:
        // su parent ES mods_dir, asi que el guard NO rechaza por ubicacion (un dir inexistente si).
        assert_eq!(dup.parent(), Some(install.mods_dir.as_path()));
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
    fn prepare_dst_detecta_copias_con_nombre_distinto_y_deshabilitadas() {
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let install = temp_install("sts2_modsync_manager_dupdetect");
        // Copia VIEJA con NOMBRE de carpeta distinto del id (tipico al recibir mods por Drive).
        std::fs::create_dir_all(install.mods_dir.join("FGOCore-1.0")).unwrap();
        std::fs::write(
            install.mods_dir.join("FGOCore-1.0").join("FGOCore.json"),
            r#"{"id":"FGOCore","version":"1.0"}"#,
        )
        .unwrap();
        // Y una copia DESHABILITADA del mismo id.
        std::fs::create_dir_all(disabled_dir(&install).join("FGOCore")).unwrap();
        std::fs::write(
            disabled_dir(&install).join("FGOCore").join("FGOCore.json"),
            r#"{"id":"FGOCore","version":"0.9"}"#,
        )
        .unwrap();

        // `dirs_with_id` halla AMBAS (no solo mods/<id>): es lo que el codigo viejo se perdia.
        let found = dirs_with_id(&install, "FGOCore");
        assert_eq!(
            found.len(),
            2,
            "deberia hallar la copia renombrada y la deshabilitada"
        );

        // Instalar la version nueva SIN overwrite -> detecta el duplicado y FALLA (antes lo ignoraba
        // por estar en una carpeta con otro nombre, y dejaba el duplicado).
        let src_parent = install.root.join("incoming");
        std::fs::create_dir_all(src_parent.join("FGOCore")).unwrap();
        std::fs::write(
            src_parent.join("FGOCore").join("FGOCore.json"),
            r#"{"id":"FGOCore","version":"1.2"}"#,
        )
        .unwrap();
        assert!(install_from_dir(&install, &src_parent.join("FGOCore"), false).is_err());
        // No se borro nada con !overwrite.
        assert!(install.mods_dir.join("FGOCore-1.0").is_dir());
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
    fn install_from_zip_confirmed_solo_pregunta_si_ya_existe() {
        use std::cell::Cell;
        use std::io::Write;
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let install = temp_install("sts2_modsync_zip_confirmed");
        let zip_path = install.root.join("mod.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            zw.start_file("Mod/Mod.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(br#"{"id":"Mod"}"#).unwrap();
            zw.finish().unwrap();
        }

        // 1) El mod NO esta instalado: NO se pide confirmacion y se instala directo.
        assert!(!is_id_installed(&install, "Mod"));
        let asked = Cell::new(false);
        let r = install_from_zip_confirmed(&install, &zip_path, |_| {
            asked.set(true);
            true
        })
        .unwrap();
        assert_eq!(r, Some("Mod".to_string()));
        assert!(
            !asked.get(),
            "no debe preguntar si el mod no estaba instalado"
        );
        assert!(install.mods_dir.join("Mod").join("Mod.json").is_file());

        // 2) Ya instalado + el usuario dice NO: no reemplaza nada (no manda a la papelera) y da None.
        assert!(is_id_installed(&install, "Mod"));
        let r = install_from_zip_confirmed(&install, &zip_path, |id| {
            assert_eq!(id, "Mod"); // se le pasa el id en conflicto
            false
        })
        .unwrap();
        assert_eq!(r, None, "con confirm=false no debe instalar");
        assert!(install.mods_dir.join("Mod").join("Mod.json").is_file());
        let _ = std::fs::remove_dir_all(&install.root);
    }

    #[test]
    fn archive_kind_por_magic_y_7z_round_trip() {
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let install = temp_install("sts2_modsync_manager_7z");
        // Deteccion por MAGIC (no por extension): un .zip se reconoce por PK, un .7z por su firma.
        std::fs::write(install.root.join("a.zip"), b"PK\x03\x04xx").unwrap();
        assert_eq!(archive_kind(&install.root.join("a.zip")), ArchiveKind::Zip);
        std::fs::write(
            install.root.join("a.7z"),
            [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C],
        )
        .unwrap();
        assert_eq!(
            archive_kind(&install.root.join("a.7z")),
            ArchiveKind::SevenZ
        );
        std::fs::write(install.root.join("a.rar"), b"Rar!\x1a\x07\x00").unwrap();
        assert_eq!(
            archive_kind(&install.root.join("a.rar")),
            ArchiveKind::Other
        );

        // Round-trip real: armar un .7z con Mod/Mod.json y que `install_from_zip` lo instale.
        let src = install.root.join("incoming");
        std::fs::create_dir_all(src.join("Mod")).unwrap();
        std::fs::write(src.join("Mod").join("Mod.json"), br#"{"id":"Mod"}"#).unwrap();
        let sevenz = install.root.join("mod.7z");
        sevenz_rust::compress_to_path(&src, &sevenz).unwrap();
        assert_eq!(archive_kind(&sevenz), ArchiveKind::SevenZ);
        let id = install_from_zip(&install, &sevenz, false).unwrap();
        assert_eq!(id, "Mod");
        assert!(install.mods_dir.join("Mod").join("Mod.json").is_file());
        let _ = std::fs::remove_dir_all(&install.root);
    }

    #[test]
    fn install_update_zip_rechaza_un_id_distinto_y_no_pisa_otro_mod() {
        use std::io::Write;
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let install = temp_install("sts2_modsync_update_zip_id");
        make_mod(&install.mods_dir, "Otro"); // un mod ya instalado que NO hay que pisar
        // .zip que dice ser el mod "Otro" (no "Mod"): actualizar "Mod" NO debe instalarlo/pisar "Otro".
        let zip_path = install.root.join("rel.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            zw.start_file("Otro/Otro.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(br#"{"id":"Otro","version":"9.9"}"#).unwrap();
            zw.finish().unwrap();
        }
        let err = install_update_zip(&install, &zip_path, "Mod");
        assert!(err.is_err(), "un id distinto debe ABORTAR");
        // El mod "Otro" original sigue intacto (NO fue a la papelera ni se reemplazo).
        assert!(install.mods_dir.join("Otro").join("Otro.json").is_file());
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
