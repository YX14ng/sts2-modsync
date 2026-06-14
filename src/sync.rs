//! Sincronizacion: `plan()` compara el estado real de `mods/` contra el set-manifest y
//! produce un PLAN (que bajar, que ya esta al dia, que sobra); `apply()` lo ejecuta de
//! forma TRANSACCIONAL (baja a `.part` + verifica BLAKE3, recien entonces renombra; manda
//! huerfanos a la papelera). La descarga concreta la hace un `transport::ModSource`.

use crate::hashing;
use crate::manifest::{FileEntry, SetManifest};
use crate::transport::ModSource;
use anyhow::{Context, Result, bail};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Default)]
pub struct Plan {
    /// Faltan localmente o el hash difiere => hay que bajarlos.
    pub to_download: Vec<FileEntry>,
    /// Ya correctos (hash coincide) => se omiten (esto es el ahorro).
    pub up_to_date: Vec<String>,
    /// HUERFANOS: estan local DENTRO de una carpeta gestionada pero no en el
    /// manifiesto => se borran al aplicar. JAMAS incluye nada fuera de managed_ids
    /// (los mods ajenos del usuario quedan intactos).
    pub orphans: Vec<PathBuf>,
    /// Orden topologico de instalacion (dependencias primero).
    pub install_order: Vec<String>,
    /// Orden de carga CANONICO en runtime (BaseLib + A-Z) — el que alimenta el room-hash
    /// de BaseLib en multiplayer. Lo impone ModListSorter; distinto de `install_order`.
    pub load_order: Vec<String>,
    /// El set incluye `ModListSorter`? Si no, los amigos pueden quedar con otro orden de
    /// carga y no entrar al lobby (room-hash distinto).
    pub load_order_enforced: bool,
    /// Bytes totales a transferir.
    pub bytes_to_download: u64,
}

impl Plan {
    pub fn is_noop(&self) -> bool {
        self.to_download.is_empty() && self.orphans.is_empty()
    }
}

/// Calcula el plan comparando `manifest` contra el contenido real de `mods_dir`.
pub fn plan(manifest: &SetManifest, mods_dir: &Path) -> Result<Plan> {
    let mut plan = Plan {
        install_order: manifest.install_order()?,
        load_order: manifest.canonical_load_order(),
        load_order_enforced: manifest.syncs_load_order(),
        ..Default::default()
    };

    // 1) Que bajar vs que ya esta (hash por archivo = capa delta gruesa).
    let mut expected: BTreeSet<PathBuf> = BTreeSet::new();
    for m in &manifest.mods {
        for f in &m.files {
            let local = mods_dir.join(rel_to_native(&f.path));
            expected.insert(local.clone());
            if hashing::matches(&local, &f.blake3) {
                plan.up_to_date.push(f.path.clone());
            } else {
                plan.bytes_to_download += f.size;
                plan.to_download.push(f.clone());
            }
        }
    }

    // 2) Huerfanos, acotados a las carpetas gestionadas (managed_ids).
    for id in manifest.managed_ids() {
        let dir = mods_dir.join(&id);
        if !dir.is_dir() {
            continue;
        }
        for entry in WalkDir::new(&dir).into_iter().filter_map(Result::ok) {
            if entry.file_type().is_file() && !expected.contains(entry.path()) {
                plan.orphans.push(entry.path().to_path_buf());
            }
        }
    }

    Ok(plan)
}

/// "a/b/c" -> PathBuf con separadores nativos.
fn rel_to_native(p: &str) -> PathBuf {
    p.split(['/', '\\']).collect()
}

/// Aplica el plan de forma TRANSACCIONAL (ver HANDOFF.md §sync):
///  1. aborta si el juego corre (lock de .dll/.pck en Windows);
///  2. baja cada `to_download` a `<dest>.part` (via `source`) y verifica su BLAKE3 —
///     si algo falla, NO se renombro nada todavia (los `.part` quedan, se rehacen);
///  3. SOLO si TODO verifico, renombra los `.part` -> destino (rapido) en orden
///     topologico (libs antes que dependientes);
///  4. manda los huerfanos a la papelera (reversible) tras el exito.
///
/// `on_progress` recibe el total de bytes bajados acumulado (para la barra de progreso).
pub fn apply(
    plan: &Plan,
    manifest: &SetManifest,
    mods_dir: &Path,
    source: &dyn ModSource,
    on_progress: &mut dyn FnMut(u64),
) -> Result<()> {
    if crate::detect::is_game_running() {
        bail!("El juego esta ABIERTO — cerralo antes de instalar (lock de .dll/.pck).");
    }

    // 1+2) Bajar TODO a `.part` + verificar blake3. Nada se renombra hasta que todo paso.
    let rank = order_rank(&plan.install_order);
    let mut done: u64 = 0;

    // Pre-carga opcional del set entero (torrent: se une al swarm y baja todo junto). Las
    // fuentes por-archivo (HTTP) lo ignoran (default no-op). Los bytes que reporte aca NO se
    // recuentan en el loop de fetch.
    source.prepare(&plan.to_download, &mut |n| {
        done += n;
        on_progress(done);
    })?;

    let mut staged: Vec<(PathBuf, PathBuf, usize)> = Vec::new(); // (part, dest, rank topologico)
    for entry in &plan.to_download {
        let dest = mods_dir.join(rel_to_native(&entry.path));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creando {}", parent.display()))?;
        }
        let part = part_path(&dest);
        source.fetch(&manifest.base_url, entry, &part, &mut |n| {
            done += n;
            on_progress(done);
        })?;
        if !hashing::matches(&part, &entry.blake3) {
            let _ = std::fs::remove_file(&part);
            bail!(
                "el BLAKE3 no coincide tras bajar {} (asset corrupto o equivocado)",
                entry.path
            );
        }
        let mod_id = entry.path.split(['/', '\\']).next().unwrap_or("");
        let r = rank.get(mod_id).copied().unwrap_or(usize::MAX);
        staged.push((part, dest, r));
    }

    // El juego pudo ABRIRSE durante la descarga (lock de .dll/.pck en Windows). Re-chequear
    // antes de renombrar: si esta abierto, abortar sin tocar nada (los .part quedan para reintentar).
    if crate::detect::is_game_running() {
        bail!("el juego se abrio durante la descarga — cerralo y reintenta (no se instalo nada)");
    }

    // 3) Todo verificado -> renombrar (casi atomico, raramente falla) en orden topologico.
    staged.sort_by_key(|&(_, _, r)| r);
    for (part, dest, _) in &staged {
        std::fs::rename(part, dest).with_context(|| format!("renombrando a {}", dest.display()))?;
    }

    // 4) Huerfanos a la papelera (reversible) tras el exito.
    for orphan in &plan.orphans {
        let _ = trash::delete(orphan);
    }
    Ok(())
}

/// `<dest>` + ".part" (preserva la extension original: BaseLib.dll -> BaseLib.dll.part).
fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

/// Mapa id-de-mod -> posicion en el orden topologico de instalacion.
fn order_rank(order: &[String]) -> HashMap<&str, usize> {
    order
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{FileEntry, ModEntry, SetManifest};

    fn content_for(path: &str) -> Vec<u8> {
        format!("contenido de {path}").into_bytes()
    }

    fn entry(path: &str) -> FileEntry {
        let c = content_for(path);
        FileEntry {
            path: path.into(),
            size: c.len() as u64,
            blake3: blake3::hash(&c).to_hex().to_string(),
        }
    }

    fn manifest_one(mod_id: &str, file: &str) -> SetManifest {
        SetManifest {
            schema: 1,
            set_name: "t".into(),
            set_version: "1".into(),
            published_at: "now".into(),
            signing_key_id: None,
            base_url: "https://example/".into(),
            magnet: None,
            baselib_version: None,
            mods: vec![ModEntry {
                id: mod_id.into(),
                version: "1".into(),
                dependencies: vec![],
                files: vec![entry(file)],
            }],
        }
    }

    /// "Baja" escribiendo el contenido cuyo hash conoce el manifiesto (download exitoso).
    struct GoodSource;
    impl ModSource for GoodSource {
        fn fetch(
            &self,
            _base: &str,
            entry: &FileEntry,
            dest: &Path,
            on_bytes: &mut dyn FnMut(u64),
        ) -> Result<()> {
            let c = content_for(&entry.path);
            std::fs::write(dest, &c)?;
            on_bytes(c.len() as u64);
            Ok(())
        }
    }

    /// Escribe contenido EQUIVOCADO -> debe fallar la verificacion blake3.
    struct BadSource;
    impl ModSource for BadSource {
        fn fetch(
            &self,
            _base: &str,
            _entry: &FileEntry,
            dest: &Path,
            on_bytes: &mut dyn FnMut(u64),
        ) -> Result<()> {
            std::fs::write(dest, b"basura")?;
            on_bytes(6);
            Ok(())
        }
    }

    #[test]
    fn apply_baja_verifica_y_renombra() {
        // apply() aborta si el juego corre; este test necesita el juego cerrado.
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let base = std::env::temp_dir().join("sts2_modsync_apply_ok");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let manifest = manifest_one("Mod", "Mod/a.txt");
        let plan = plan(&manifest, &base).unwrap();
        assert_eq!(plan.to_download.len(), 1);

        let mut total = 0u64;
        apply(&plan, &manifest, &base, &GoodSource, &mut |d| total = d).unwrap();

        let landed = base.join("Mod").join("a.txt");
        assert!(landed.is_file());
        assert_eq!(std::fs::read(&landed).unwrap(), content_for("Mod/a.txt"));
        assert!(!part_path(&landed).exists()); // el .part ya no esta
        assert_eq!(total, content_for("Mod/a.txt").len() as u64);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_falla_si_el_hash_no_coincide_y_no_escribe_destino() {
        let base = std::env::temp_dir().join("sts2_modsync_apply_bad");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let manifest = manifest_one("Mod", "Mod/a.txt");
        let plan = plan(&manifest, &base).unwrap();

        let err = apply(&plan, &manifest, &base, &BadSource, &mut |_| {});
        assert!(err.is_err());
        // el destino NO se creo (solo habia un .part, que se borro al fallar la verificacion).
        assert!(!base.join("Mod").join("a.txt").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
