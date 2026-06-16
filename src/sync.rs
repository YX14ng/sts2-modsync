//! Sincronizacion: `plan()` compara el estado real de `mods/` contra el set-manifest y
//! produce un PLAN (que bajar, que ya esta al dia, que sobra); `apply()` lo ejecuta de
//! forma TRANSACCIONAL (baja a `.part` + verifica BLAKE3, recien entonces renombra; manda
//! huerfanos a la papelera). La descarga concreta la hace un `transport::ModSource`.

use crate::detect::Install;
use crate::hashing;
use crate::manifest::{Delta, FileEntry, SetManifest};
use crate::transport::ModSource;
use anyhow::{Context, Result, bail};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use walkdir::WalkDir;

/// Tope de tamaño (del archivo NUEVO) para usar un delta en el cliente: aplicar un patch tiene vivos
/// a la vez el viejo + el patch + el resultado en RAM (~3x el archivo), asi que arriba de esto
/// conviene el full (que se baja en streaming a disco, memoria baja). Los `.pck` tipicos de mods van
/// debajo; a 256 MB el pico de RAM es ~768 MB, aceptable; mas grande no vale la pena.
const DELTA_CLIENT_MAX: u64 = 256 * 1024 * 1024;

/// Un archivo a transferir: el asset COMPLETO, o —si el cliente ya tiene una version anterior que
/// algun `delta.from_blake3` matchea— un PATCH bsdiff (mucho mas chico) que se aplica sobre el viejo.
#[derive(Debug, Clone)]
pub struct Download {
    /// El archivo destino con su `blake3`/`size`/`path` del manifest (lo que tiene que quedar).
    pub entry: FileEntry,
    /// Si `Some`, bajar este patch y aplicarlo sobre el archivo local actual (delta); si `None`,
    /// bajar el asset completo. Si el delta falla en cualquier paso, `apply` cae al full.
    pub delta: Option<Delta>,
}

impl Download {
    /// Bytes que se transfieren por la red para este archivo: el patch si hay delta, si no el full.
    pub fn fetch_bytes(&self) -> u64 {
        self.delta
            .as_ref()
            .map_or(self.entry.size, |d| d.patch_size)
    }

    /// `true` si se va a transferir un patch (delta) en vez del archivo completo.
    pub fn is_delta(&self) -> bool {
        self.delta.is_some()
    }
}

#[derive(Debug, Default)]
pub struct Plan {
    /// Faltan localmente o el hash difiere => hay que bajarlos (full o patch).
    pub to_download: Vec<Download>,
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

    /// Pico de espacio en disco que la descarga necesita. NO alcanza con `bytes_to_download` (que
    /// para un delta cuenta el patch, chico): cada `.part` que se materializa tiene el tamaño FULL
    /// del archivo (un delta reconstruye el archivo ENTERO en el `.part`; un fallback baja el full),
    /// y todos los `.part` coexisten hasta el commit. Por eso el pre-check de disco usa esta suma.
    pub fn install_bytes(&self) -> u64 {
        self.to_download.iter().map(|d| d.entry.size).sum()
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

    // 1) Que bajar vs que ya esta (hash por archivo = capa delta gruesa). El cache evita
    // re-hashear los `.pck` que no cambiaron (compara size+mtime); se persiste en %APPDATA%.
    let mut cache = hashing::HashCache::load();
    // Claves (normalizadas) de los archivos del manifest, para detectar huerfanos. NO se guarda el
    // PathBuf crudo: en Windows el FS es case-insensitive, asi un `Mod/BaseLib.pck` en disco que el
    // manifest declara como `Mod/baselib.pck` matchea igual y NO se manda a la papelera (ver `orphan_key`).
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for m in &manifest.mods {
        for f in &m.files {
            let local = mods_dir.join(rel_to_native(&f.path));
            expected.insert(orphan_key(&local));
            if cache.matches(&local, &f.blake3) {
                plan.up_to_date.push(f.path.clone());
            } else {
                // ¿Hay un PATCH chico aplicable (el cliente ya tiene una version vieja que matchea)?
                // Si si, se transfiere el patch en vez del full; si no, el full.
                let delta = pick_delta(f, &local, &mut cache);
                plan.bytes_to_download += delta.as_ref().map_or(f.size, |d| d.patch_size);
                plan.to_download.push(Download {
                    entry: f.clone(),
                    delta,
                });
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
            let p = entry.path();
            // Los `.part` son descargas en curso/abortadas, NO huerfanos (se barren aparte).
            if entry.file_type().is_file() && !is_part_file(p) && !expected.contains(&orphan_key(p))
            {
                plan.orphans.push(p.to_path_buf());
            }
        }
    }

    cache.save(); // persistir los hashes nuevos para el proximo plan
    Ok(plan)
}

/// "a/b/c" -> PathBuf con separadores nativos.
fn rel_to_native(p: &str) -> PathBuf {
    p.split(['/', '\\']).collect()
}

/// Clave para comparar una ruta al detectar huerfanos. En Windows el filesystem es
/// case-INSENSITIVE: si el manifest declara `Mod/baselib.pck` pero en disco esta como
/// `Mod/BaseLib.pck`, comparar exacto haria que el archivo (que ES el querido) no matchee y se
/// mande a la papelera, rompiendo el mod. Por eso ahi normalizamos a minusculas; en el resto de
/// plataformas (case-sensitive) se deja igual. Ambos lados de la comparacion usan esta misma clave.
fn orphan_key(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if cfg!(windows) { s.to_lowercase() } else { s }
}

/// Elige un patch para `f` si el archivo LOCAL viejo existe y su BLAKE3 matchea algun
/// `delta.from_blake3` (y el patch es mas chico que el full, y el archivo no es enorme). `None` =>
/// bajar el full. El `local_hash` sale del cache (ya lo calculo `matches`), asi no se re-hashea.
fn pick_delta(f: &FileEntry, local: &Path, cache: &mut hashing::HashCache) -> Option<Delta> {
    if f.deltas.is_empty() || f.size > DELTA_CLIENT_MAX || !local.is_file() {
        return None;
    }
    let local_hash = cache.blake3(local).ok()?;
    f.deltas
        .iter()
        .find(|d| d.from_blake3.eq_ignore_ascii_case(&local_hash) && d.patch_size < f.size)
        .cloned()
}

/// Resultado de `apply`: nada se "traga" en silencio. Si algun huerfano no se pudo mandar a
/// la papelera, se reporta aca (el set quedo instalado igual, pero hay basura que avisar).
#[derive(Debug, Default)]
pub struct ApplyReport {
    /// Archivos instalados (renombrados desde su `.part`).
    pub installed: usize,
    /// Huerfanos que NO se pudieron borrar (se reportan en vez de tragarse el error).
    pub orphans_failed: Vec<PathBuf>,
}

/// Intentos de descarga+verificacion por archivo antes de rendirse. Si el `.part` quedo
/// corrupto (resume sobre basura), se borra y se baja de cero en el siguiente intento.
const FETCH_ATTEMPTS: u32 = 2;

/// Aplica el plan de forma TRANSACCIONAL (ver HANDOFF.md §sync):
///  1. aborta si el juego corre (lock de .dll/.pck en Windows) o si no entra en disco;
///  2. baja cada `to_download` a `<dest>.part` (via `source`) y verifica su BLAKE3,
///     reintentando DE CERO si el `.part` quedo corrupto — si algo falla, NO se renombro
///     nada todavia (los `.part` validos quedan para reanudar);
///  3. SOLO si TODO verifico, re-chequea el juego y renombra los `.part` -> destino en orden
///     topologico con BACKUP + ROLLBACK: si un rename falla a mitad, deshace los ya hechos y
///     restaura los archivos viejos (el set nunca queda a medio aplicar);
///  4. manda los huerfanos a la papelera (reversible) tras el exito, REPORTA los que no se
///     pudieron borrar, y barre los `.part` que hayan quedado.
///
/// `on_progress` recibe el total de bytes bajados acumulado (para la barra). `on_file` recibe
/// el `path` del archivo que se empieza a bajar (para "bajando X..."). `cancel` se chequea
/// entre archivos Y durante cada descarga: si se setea, aborta limpio (nada se instalo; los
/// `.part` quedan para reanudar despues).
pub fn apply(
    plan: &Plan,
    manifest: &SetManifest,
    mods_dir: &Path,
    source: &dyn ModSource,
    on_progress: &mut dyn FnMut(u64),
    on_file: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<ApplyReport> {
    if crate::detect::is_game_running() {
        bail!("El juego esta ABIERTO — cerralo antes de instalar (lock de .dll/.pck).");
    }

    // Pre-check de disco (best-effort): no arrancar una descarga que no va a entrar. Usa el tamaño
    // FULL de lo que se baja (no `bytes_to_download`, que para los deltas cuenta solo el patch): cada
    // `.part` materializa el archivo entero, y un delta que aplica/cae al full igual escribe el full.
    let disk_needed = plan.install_bytes();
    if disk_needed > 0
        && let Some(free) = free_space_for(mods_dir)
    {
        let margin = (disk_needed / 20).max(64 * 1024 * 1024);
        let need = disk_needed.saturating_add(margin);
        if free < need {
            bail!(
                "espacio insuficiente en disco: hacen falta ~{} MB y hay {} MB libres",
                need / 1_048_576,
                free / 1_048_576
            );
        }
    }

    let rank = order_rank(&plan.install_order);
    let mut done: u64 = 0;

    // Pre-carga opcional del set entero (torrent: se une al swarm y baja todo junto). Las
    // fuentes por-archivo (HTTP) lo ignoran (default no-op). Los bytes que reporte aca NO se
    // recuentan en el loop de fetch. Se le pasa lo que REALMENTE se va a bajar (patch o full).
    let fetch_targets: Vec<FileEntry> = plan.to_download.iter().map(fetch_target).collect();
    source.prepare(&fetch_targets, &mut |n| {
        done += n;
        on_progress(done);
        !cancel.load(Ordering::Relaxed)
    })?;

    // 1+2) Bajar TODO a `.part` + verificar blake3 (reintenta de cero si quedo corrupto). Para los
    // archivos con delta: bajar el patch, aplicarlo sobre el viejo, verificar; si falla, full.
    let mut staged: Vec<(PathBuf, PathBuf, usize)> = Vec::new(); // (part, dest, rank topologico)
    for download in &plan.to_download {
        if cancel.load(Ordering::Relaxed) {
            bail!("sincronizacion cancelada (no se instalo nada)");
        }
        let entry = &download.entry;
        on_file(&entry.path);
        let dest = mods_dir.join(rel_to_native(&entry.path));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(long_path(parent).as_ref())
                .with_context(|| format!("creando {}", parent.display()))?;
        }
        let part = part_path(&dest);
        // Camino delta (si lo hay): exito => `.part` listo; si no se pudo, cae al full mas abajo.
        let installed_via_delta = if download.delta.is_some() {
            try_apply_delta(
                source,
                &manifest.base_url,
                download,
                &dest,
                &part,
                &mut done,
                on_progress,
                cancel,
            )?
        } else {
            false
        };
        if !installed_via_delta {
            fetch_verified(
                source,
                &manifest.base_url,
                entry,
                &part,
                &mut done,
                on_progress,
                cancel,
            )?;
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

    // 3) Todo verificado -> renombrar en orden topologico, con backup + rollback transaccional.
    staged.sort_by_key(|&(_, _, r)| r);
    commit_staged(&staged, mods_dir)?;

    // 4) Huerfanos a la papelera (reversible); reportar los que fallen (no tragar el error).
    let mut report = ApplyReport {
        installed: staged.len(),
        orphans_failed: Vec::new(),
    };
    for orphan in &plan.orphans {
        if trash::delete(orphan).is_err() {
            report.orphans_failed.push(orphan.clone());
        }
    }
    sweep_parts(manifest, mods_dir); // limpiar `.part` que hayan quedado de intentos abortados
    Ok(report)
}

/// Baja `entry` a `part` y verifica su BLAKE3, reintentando DE CERO si quedo corrupto (resume
/// sobre basura -> hash distinto). El progreso del intento fallido se revierte para no inflar
/// la barra. Borra el `.part` si se rinde (no deja basura que un resume futuro reanude).
fn fetch_verified(
    source: &dyn ModSource,
    base_url: &str,
    entry: &FileEntry,
    part: &Path,
    done: &mut u64,
    on_progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<()> {
    let part_io = long_path(part);
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=FETCH_ATTEMPTS {
        // Si se cancelo (posiblemente a mitad de un fetch), salir YA sin truncar ni borrar el
        // `.part`: se preserva lo bajado para reanudar despues (lo que promete la doc/UI).
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("sincronizacion cancelada"));
        }
        if attempt > 1 {
            // Reintento -> de cero: TRUNCAR el `.part` a 0 (no solo borrar) para que transport
            // NUNCA reanude (Range) sobre datos corruptos. `File::create` deja tamano 0 ->
            // transport ve `existing==0` y baja entero. Si ni eso se puede, el hash igual atrapa.
            let _ = std::fs::File::create(part_io.as_ref());
        }
        let base = *done;
        let res = source.fetch(base_url, entry, part_io.as_ref(), &mut |n| {
            *done += n;
            on_progress(*done);
            !cancel.load(Ordering::Relaxed)
        });
        match res {
            Ok(()) if hashing::matches(part_io.as_ref(), &entry.blake3) => return Ok(()),
            Ok(()) => {
                last_err = Some(anyhow::anyhow!(
                    "el BLAKE3 no coincide tras bajar {} (asset corrupto o equivocado)",
                    entry.path
                ));
            }
            Err(e) => last_err = Some(e),
        }
        // Revertir el progreso del intento fallido (la barra no debe avanzar por basura).
        *done = base;
        on_progress(*done);
    }
    let _ = std::fs::remove_file(part_io.as_ref());
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no se pudo bajar {}", entry.path)))
}

/// El asset que se baja REALMENTE para un `Download`: el patch (content-addressed por su propio
/// blake3) si hay delta, o el archivo full. Se usa para `prepare` (P2P) y para identificar el asset.
fn fetch_target(d: &Download) -> FileEntry {
    match &d.delta {
        Some(delta) => FileEntry {
            path: format!("{} [delta]", d.entry.path),
            size: delta.patch_size,
            blake3: delta.patch_blake3.clone(),
            deltas: Vec::new(),
        },
        None => d.entry.clone(),
    }
}

/// Subdir/temp para el patch bsdiff que se esta bajando. Termina en `.part` para que `is_part_file`
/// lo trate como descarga en curso (excluido de huerfanos, barrido por `sweep_parts`).
fn dpatch_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".dpatch.part");
    PathBuf::from(s)
}

/// Intenta instalar `download` via DELTA (cliente): lee el archivo VIEJO local, baja el patch
/// (verificando su propio BLAKE3), lo aplica, y verifica que el RESULTADO tenga el blake3 esperado;
/// si todo va bien deja el nuevo contenido en `part` (listo para el rename transaccional). Devuelve
/// `Ok(true)` si el delta se aplico y verifico (`part` listo); `Ok(false)` si NO se pudo usar (viejo
/// ausente/cambiado, patch fallo, o el hash del resultado no coincide) -> el llamador baja el asset
/// COMPLETO y se revierte el progreso del intento; `Err(..)` si el usuario cancelo (abortar).
///
/// Es seguro por construccion: el resultado SIEMPRE se verifica por BLAKE3 antes de aceptarlo, asi
/// un patch malo nunca instala bytes equivocados (cae al full).
#[allow(clippy::too_many_arguments)]
fn try_apply_delta(
    source: &dyn ModSource,
    base_url: &str,
    download: &Download,
    dest: &Path,
    part: &Path,
    done: &mut u64,
    on_progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<bool> {
    let Some(delta) = &download.delta else {
        return Ok(false);
    };
    let entry = &download.entry;

    // 1) El archivo VIEJO local tiene que existir y matchear `from_blake3` (pudo cambiar desde el
    //    plan). Si no, no se puede aplicar este patch -> full.
    let Ok(old) = std::fs::read(long_path(dest).as_ref()) else {
        return Ok(false);
    };
    if !hashing::blake3_bytes(&old).eq_ignore_ascii_case(&delta.from_blake3) {
        return Ok(false);
    }

    // 2) Bajar el patch a un temp, verificando su PROPIO blake3 (asset content-addressed).
    let patch_part = dpatch_path(dest);
    let patch_entry = FileEntry {
        path: format!("{} [delta]", entry.path),
        size: delta.patch_size,
        blake3: delta.patch_blake3.clone(),
        deltas: Vec::new(),
    };
    let base = *done;
    let fetched = fetch_verified(
        source,
        base_url,
        &patch_entry,
        &patch_part,
        done,
        on_progress,
        cancel,
    );
    if cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(long_path(&patch_part).as_ref());
        bail!("sincronizacion cancelada");
    }
    if fetched.is_err() {
        *done = base; // revertir el progreso del patch que no se pudo bajar
        on_progress(*done);
        let _ = std::fs::remove_file(long_path(&patch_part).as_ref());
        return Ok(false); // -> full
    }

    // 3) Aplicar el patch sobre el viejo y verificar el RESULTADO contra el blake3 del manifest.
    let applied = std::fs::read(long_path(&patch_part).as_ref())
        .map_err(anyhow::Error::new)
        .and_then(|patch_bytes| crate::delta::apply(&old, &patch_bytes, entry.size));
    let _ = std::fs::remove_file(long_path(&patch_part).as_ref()); // el patch ya no se necesita
    let new = match applied {
        Ok(n) if hashing::blake3_bytes(&n).eq_ignore_ascii_case(&entry.blake3) => n,
        _ => {
            // patch corrupto, viejo equivocado, o el resultado no matchea -> revertir y caer al full.
            *done = base;
            on_progress(*done);
            return Ok(false);
        }
    };

    // 4) Dejar el nuevo contenido en `.part` (el commit lo renombra como cualquier descarga).
    std::fs::write(long_path(part).as_ref(), &new)
        .with_context(|| format!("escribiendo {}", part.display()))?;
    Ok(true)
}

/// Subcarpeta reservada dentro de `mods/` para los backups del commit. NO es un mod (el
/// orphan-scan solo recorre managed_ids) y `manifest::validate_ids` rechaza un mod con `:`/
/// separadores, pero este nombre lleva `.` inicial: igual no colisiona porque el backup vive
/// en su PROPIO subdir con nombres `bak-<n>` que ningun `dest` del manifest puede producir.
const BACKUP_DIR: &str = ".modsync-backup";

/// Renombra cada `.part` a su destino con BACKUP + ROLLBACK: respalda el archivo viejo (si
/// habia) en `mods/.modsync-backup/bak-<n>` antes de pisarlo y, si algun rename falla, deshace
/// TODO (restaura los viejos, saca los nuevos) para no dejar el set a medio aplicar. Recien al
/// final descarta los backups. Si el rollback NO pudo restaurar algun viejo, NO borra los
/// backups y lo REPORTA en el error (no se traga). Recuperacion ante CRASH a mitad del commit
/// queda fuera de alcance (ver ROADMAP 0.6).
fn commit_staged(staged: &[(PathBuf, PathBuf, usize)], mods_dir: &Path) -> Result<()> {
    let backup_dir = mods_dir.join(BACKUP_DIR);
    // journal: (dest, backup-del-viejo-si-habia) de cada swap ya aplicado, para deshacer.
    let mut journal: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let mut next: u64 = 0;
    for (part, dest, _) in staged {
        match swap_in(part, dest, &backup_dir, &mut next) {
            Ok(backup) => journal.push((dest.clone(), backup)),
            Err(e) => {
                let failed = rollback(&journal);
                if failed.is_empty() {
                    discard_backups(&journal, &backup_dir); // estado viejo restaurado entero
                    return Err(e.context(format!("instalando {}", dest.display())));
                }
                return Err(e.context(format!(
                    "instalando {} y NO se pudieron restaurar {} archivo(s): los originales quedaron en {}",
                    dest.display(),
                    failed.len(),
                    backup_dir.display()
                )));
            }
        }
    }
    discard_backups(&journal, &backup_dir); // exito: descartar los respaldos
    Ok(())
}

/// Pisa `dest` con `part` de forma reversible: si `dest` existia, lo mueve a un backup unico en
/// `backup_dir` (que devuelve) antes de renombrar `part`->`dest`. Si el rename falla, restaura el
/// backup. Usa `dest_io` (long-path) para TODAS las comprobaciones y renames: el `exists()` DEBE
/// mirar el mismo path verbatim que el rename, sino en rutas >260 sin long-path-OS daria un falso
/// negativo y se pisaria el viejo sin respaldo.
fn swap_in(part: &Path, dest: &Path, backup_dir: &Path, next: &mut u64) -> Result<Option<PathBuf>> {
    let dest_io = long_path(dest);
    let backup = if dest_io.exists() {
        std::fs::create_dir_all(long_path(backup_dir).as_ref())
            .with_context(|| format!("creando {}", backup_dir.display()))?;
        // Elegir un nombre LIBRE: un commit previo que fallo su rollback pudo dejar backups
        // (los originales del usuario) en este dir; reiniciar el contador en 0 NO debe pisarlos.
        let b = loop {
            let cand = backup_dir.join(format!("bak-{next}"));
            *next += 1;
            if !long_path(&cand).exists() {
                break cand;
            }
        };
        std::fs::rename(dest_io.as_ref(), long_path(&b).as_ref())
            .with_context(|| format!("respaldando {}", dest.display()))?;
        Some(b)
    } else {
        None
    };
    let part_io = long_path(part);
    if let Err(e) = std::fs::rename(part_io.as_ref(), dest_io.as_ref()) {
        if let Some(b) = &backup {
            let _ = std::fs::rename(long_path(b).as_ref(), dest_io.as_ref()); // restaurar el viejo
        }
        return Err(anyhow::Error::new(e));
    }
    Ok(backup)
}

/// Deshace los swaps ya aplicados (orden inverso): restaura el viejo desde su backup (el rename
/// REEMPLAZA al nuevo en Windows) o, si no habia viejo, saca el nuevo. Devuelve los `dest` que
/// NO se pudieron restaurar (para reportarlos, no tragarlos).
fn rollback(journal: &[(PathBuf, Option<PathBuf>)]) -> Vec<PathBuf> {
    let mut failed = Vec::new();
    for (dest, backup) in journal.iter().rev() {
        let dest_io = long_path(dest);
        match backup {
            // rename del backup REEMPLAZA al archivo nuevo (atomico en Windows).
            Some(b) if std::fs::rename(long_path(b).as_ref(), dest_io.as_ref()).is_ok() => {}
            Some(_) => failed.push(dest.clone()), // no se pudo restaurar el viejo
            None => {
                // no habia viejo: sacar el nuevo para volver al estado previo.
                if std::fs::remove_file(dest_io.as_ref()).is_err() && dest_io.exists() {
                    failed.push(dest.clone());
                }
            }
        }
    }
    failed
}

/// Descarta los backups del journal y borra el dir de backups si quedo vacio (best-effort).
fn discard_backups(journal: &[(PathBuf, Option<PathBuf>)], backup_dir: &Path) {
    for (_, backup) in journal {
        if let Some(b) = backup {
            let _ = std::fs::remove_file(long_path(b).as_ref());
        }
    }
    let _ = std::fs::remove_dir(long_path(backup_dir).as_ref()); // solo borra si quedo vacio
}

/// True si la ruta termina en `.part` (descarga en curso/abortada, NO huerfano).
fn is_part_file(p: &Path) -> bool {
    p.extension()
        .map(|e| e.eq_ignore_ascii_case("part"))
        .unwrap_or(false)
}

/// Borra los `.part` que hayan quedado dentro de las carpetas gestionadas (de intentos
/// abortados). Best-effort: los errores se ignoran (no es critico).
fn sweep_parts(manifest: &SetManifest, mods_dir: &Path) {
    for id in manifest.managed_ids() {
        let dir = mods_dir.join(&id);
        if !dir.is_dir() {
            continue;
        }
        for entry in WalkDir::new(&dir).into_iter().filter_map(Result::ok) {
            if entry.file_type().is_file() && is_part_file(entry.path()) {
                let _ = std::fs::remove_file(long_path(entry.path()).as_ref());
            }
        }
    }
}

/// Carpetas DUPLICADAS de un id gestionado a limpiar tras instalar: cualquier carpeta (en `mods/`
/// o `mods_disabled/`) que declare un id del set PERO no sea la canonica `mods/<id>/`. Pura
/// seleccion (no borra). Solo propone una copia si la canonica `mods/<id>/` EXISTE — nunca borra la
/// UNICA copia de un mod. Jamas mira ids fuera de `managed_ids()` (mods ajenos quedan intactos).
fn duplicate_folders_to_clean(manifest: &SetManifest, install: &Install) -> Vec<PathBuf> {
    let managed = manifest.managed_ids();
    // Reusa el escaneo+atribucion del area gestionada de `modlist` (mismo criterio que `manager`).
    crate::modlist::folders_with_declared_id(install)
        .into_iter()
        .filter(|(p, id)| {
            if !managed.contains(id) {
                return false;
            }
            // No tocar nada si no hay copia canonica (no borrar la unica copia); y la canonica
            // (`mods/<id>/`, lo que el sync acaba de instalar/dejar) se queda siempre.
            let canonical = install.mods_dir.join(id);
            canonical.is_dir() && *p != canonical
        })
        .map(|(p, _)| p)
        .collect()
}

/// Tras una sync exitosa, manda a la PAPELERA (reversible) las carpetas duplicadas de los ids del
/// set que tengan OTRO nombre que la canonica `mods/<id>/` (o esten en `mods_disabled/`). Asi un
/// amigo que ya tenia un mod en una carpeta con otro nombre (p.ej. `SuperMod-v2/`) no queda con DOS
/// copias del mismo mod cargando a la vez —lo que cambia el room-hash de multiplayer y lo deja afuera
/// del lobby—. Best-effort: devuelve las que SE pudieron mandar a la papelera. La guarda de
/// `manager::trash_mod_dir` (juego cerrado + hija directa de `mods/`/`mods_disabled/`) asegura que
/// nunca se toca nada fuera del area gestionada.
pub fn clean_duplicate_folders(manifest: &SetManifest, install: &Install) -> Vec<PathBuf> {
    duplicate_folders_to_clean(manifest, install)
        .into_iter()
        .filter(|p| crate::manager::trash_mod_dir(install, p).is_ok())
        .collect()
}

/// Espacio libre (bytes) en el volumen que contiene `path`, si se puede determinar.
/// Best-effort: `None` si no se halla el disco (el pre-check se omite, no se bloquea).
fn free_space_for(path: &Path) -> Option<u64> {
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len()) // el mount mas especifico
        .map(|d| d.available_space())
}

/// En Windows, prefija rutas absolutas largas (>~248 chars) con `\\?\` para superar el limite
/// de 260 (MAX_PATH) en mods con arboles profundos. Solo actua si hace falta, para no alterar
/// el caso comun ni las rutas cortas de los tests. En el resto de plataformas es identidad.
#[cfg(windows)]
fn long_path(p: &Path) -> std::borrow::Cow<'_, Path> {
    use std::borrow::Cow;
    let s = p.to_string_lossy();
    if s.len() < 248 || s.starts_with("\\\\?\\") || !p.is_absolute() {
        return Cow::Borrowed(p);
    }
    let s = s.replace('/', "\\");
    // UNC (`\\server\share\...`) necesita la forma `\\?\UNC\server\share\...`; una ruta con
    // letra de unidad va como `\\?\C:\...`. Anteponer `\\?\` a secas a una UNC da un prefijo
    // malformado (`\\?\\\server`) que Windows rechaza.
    let verbatim = match s.strip_prefix("\\\\") {
        Some(rest) => format!("\\\\?\\UNC\\{rest}"),
        None => format!("\\\\?\\{s}"),
    };
    Cow::Owned(PathBuf::from(verbatim))
}

#[cfg(not(windows))]
fn long_path(p: &Path) -> std::borrow::Cow<'_, Path> {
    std::borrow::Cow::Borrowed(p)
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
            deltas: Vec::new(),
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
            game_version: None,
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
            on_bytes: &mut dyn FnMut(u64) -> bool,
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
            on_bytes: &mut dyn FnMut(u64) -> bool,
        ) -> Result<()> {
            std::fs::write(dest, b"basura")?;
            on_bytes(6);
            Ok(())
        }
    }

    /// Fuente content-addressed (como un Release real): sirve cada asset por su BLAKE3. Sirve
    /// tanto fulls como patches; falla si el hash pedido no esta (para probar el fallback).
    struct MapSource(std::collections::HashMap<String, Vec<u8>>);
    impl ModSource for MapSource {
        fn fetch(
            &self,
            _base: &str,
            entry: &FileEntry,
            dest: &Path,
            on_bytes: &mut dyn FnMut(u64) -> bool,
        ) -> Result<()> {
            let bytes = self
                .0
                .get(&entry.blake3)
                .ok_or_else(|| anyhow::anyhow!("asset {} no encontrado", entry.blake3))?;
            std::fs::write(dest, bytes)?;
            on_bytes(bytes.len() as u64);
            Ok(())
        }
    }

    /// old/new realistas (bloque grande casi-identico) + el manifest con el delta correspondiente.
    fn delta_fixture() -> (Vec<u8>, Vec<u8>, Vec<u8>, SetManifest) {
        let old = b"contenido viejo del .pck de un mod ".repeat(500);
        let mut new = old.clone();
        new[100] = b'X';
        new.extend_from_slice(b" + contenido nuevo apendido al final");
        let patch = crate::delta::diff(&old, &new).unwrap();
        let manifest = SetManifest {
            schema: 1,
            set_name: "t".into(),
            set_version: "2".into(),
            published_at: "now".into(),
            signing_key_id: None,
            base_url: "https://example/".into(),
            magnet: None,
            baselib_version: None,
            game_version: None,
            mods: vec![ModEntry {
                id: "Mod".into(),
                version: "2".into(),
                dependencies: vec![],
                files: vec![FileEntry {
                    path: "Mod/big.pck".into(),
                    size: new.len() as u64,
                    blake3: crate::hashing::blake3_bytes(&new),
                    deltas: vec![Delta {
                        from_blake3: crate::hashing::blake3_bytes(&old),
                        patch_blake3: crate::hashing::blake3_bytes(&patch),
                        patch_size: patch.len() as u64,
                    }],
                }],
            }],
        };
        (old, new, patch, manifest)
    }

    #[test]
    fn apply_usa_delta_y_reconstruye_la_version_nueva() {
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let base = std::env::temp_dir().join("sts2_modsync_delta_ok");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Mod")).unwrap();
        let (old, new, patch, manifest) = delta_fixture();
        // El cliente ya tiene la version VIEJA en disco.
        std::fs::write(base.join("Mod").join("big.pck"), &old).unwrap();

        let plan = plan(&manifest, &base).unwrap();
        assert_eq!(plan.to_download.len(), 1);
        assert!(
            plan.to_download[0].is_delta(),
            "deberia elegir el delta (el cliente tiene la version vieja)"
        );
        assert_eq!(
            plan.bytes_to_download,
            patch.len() as u64,
            "deberia transferir solo el patch, no el full"
        );
        assert!(
            (patch.len() as u64) < new.len() as u64,
            "el patch deberia ser mas chico que el full"
        );

        // Fuente que sirve el patch (y el full, por si cae) por su hash.
        let mut assets = std::collections::HashMap::new();
        assets.insert(crate::hashing::blake3_bytes(&patch), patch.clone());
        assets.insert(crate::hashing::blake3_bytes(&new), new.clone());
        let source = MapSource(assets);

        let report = apply(
            &plan,
            &manifest,
            &base,
            &source,
            &mut |_| {},
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.installed, 1);
        assert_eq!(
            std::fs::read(base.join("Mod").join("big.pck")).unwrap(),
            new,
            "el delta no reconstruyo el archivo nuevo"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_cae_al_full_si_el_patch_no_esta() {
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let base = std::env::temp_dir().join("sts2_modsync_delta_fallback");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Mod")).unwrap();
        let (old, new, _patch, manifest) = delta_fixture();
        std::fs::write(base.join("Mod").join("big.pck"), &old).unwrap();

        let plan = plan(&manifest, &base).unwrap();
        assert!(plan.to_download[0].is_delta());

        // Fuente SIN el patch (solo el full): apply debe caer al full y reconstruir igual.
        let mut assets = std::collections::HashMap::new();
        assets.insert(crate::hashing::blake3_bytes(&new), new.clone());
        let source = MapSource(assets);

        let report = apply(
            &plan,
            &manifest,
            &base,
            &source,
            &mut |_| {},
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.installed, 1);
        assert_eq!(
            std::fs::read(base.join("Mod").join("big.pck")).unwrap(),
            new,
            "el fallback al full no instalo la version nueva"
        );
        let _ = std::fs::remove_dir_all(&base);
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
        let report = apply(
            &plan,
            &manifest,
            &base,
            &GoodSource,
            &mut |d| total = d,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.installed, 1);
        assert!(report.orphans_failed.is_empty());

        let landed = base.join("Mod").join("a.txt");
        assert!(landed.is_file());
        assert_eq!(std::fs::read(&landed).unwrap(), content_for("Mod/a.txt"));
        assert!(!part_path(&landed).exists()); // el .part ya no esta
        assert_eq!(total, content_for("Mod/a.txt").len() as u64);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_cancelado_no_instala_nada() {
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let base = std::env::temp_dir().join("sts2_modsync_apply_cancel");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let manifest = manifest_one("Mod", "Mod/a.txt");
        let plan = plan(&manifest, &base).unwrap();
        let cancel = AtomicBool::new(true); // ya cancelado antes de empezar
        let err = apply(
            &plan,
            &manifest,
            &base,
            &GoodSource,
            &mut |_| {},
            &mut |_| {},
            &cancel,
        );
        assert!(err.is_err(), "con cancel seteado debe abortar");
        assert!(!base.join("Mod").join("a.txt").exists()); // no instalo nada
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Falla la verificacion la 1ra vez (escribe basura en APPEND, simulando un `.part`
    /// corrupto que un resume reanudaria) y acierta la 2da. Prueba que `fetch_verified`
    /// borra el `.part` y baja DE CERO en el reintento (sino el contenido correcto quedaria
    /// pegado a la basura y el hash seguiria mal).
    struct FlakySource {
        calls: std::sync::atomic::AtomicU32,
    }
    impl ModSource for FlakySource {
        fn fetch(
            &self,
            _base: &str,
            entry: &FileEntry,
            dest: &Path,
            on_bytes: &mut dyn FnMut(u64) -> bool,
        ) -> Result<()> {
            use std::io::Write;
            use std::sync::atomic::Ordering;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dest)?;
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                f.write_all(b"corrupto")?;
                on_bytes(8);
            } else {
                let c = content_for(&entry.path);
                f.write_all(&c)?;
                on_bytes(c.len() as u64);
            }
            Ok(())
        }
    }

    #[test]
    fn fetch_verified_reintenta_de_cero_si_quedo_corrupto() {
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let base = std::env::temp_dir().join("sts2_modsync_apply_flaky");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let manifest = manifest_one("Mod", "Mod/a.txt");
        let plan = plan(&manifest, &base).unwrap();
        let src = FlakySource {
            calls: std::sync::atomic::AtomicU32::new(0),
        };
        let report = apply(
            &plan,
            &manifest,
            &base,
            &src,
            &mut |_| {},
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.installed, 1);
        let landed = base.join("Mod").join("a.txt");
        assert_eq!(std::fs::read(&landed).unwrap(), content_for("Mod/a.txt"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn commit_staged_hace_rollback_si_falla_a_mitad() {
        let base = std::env::temp_dir().join("sts2_modsync_commit_rollback");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        // destA: archivo VIEJO + partA con el nuevo (swap OK). destB: archivo viejo pero su
        // partB NO existe -> rename(partB -> destB) falla -> rollback debe restaurar destA.
        let dest_a = base.join("a.txt");
        std::fs::write(&dest_a, b"viejo A").unwrap();
        std::fs::write(part_path(&dest_a), b"nuevo A").unwrap();
        let dest_b = base.join("b.txt");
        std::fs::write(&dest_b, b"viejo B").unwrap();
        // part_path(&dest_b) a proposito NO se crea.

        let staged = vec![
            (part_path(&dest_a), dest_a.clone(), 0usize),
            (part_path(&dest_b), dest_b.clone(), 1usize),
        ];
        assert!(commit_staged(&staged, &base).is_err());
        assert_eq!(std::fs::read(&dest_a).unwrap(), b"viejo A"); // rollback al viejo
        assert_eq!(std::fs::read(&dest_b).unwrap(), b"viejo B"); // swap_in lo restauro
        assert!(!base.join(BACKUP_DIR).exists()); // sin backups colgados (dir vaciado y borrado)
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn commit_staged_no_colisiona_con_un_dest_que_termina_como_backup() {
        // Antes los backups eran "<dest>.bak-modsync" y un set con ese path destruia datos.
        // Ahora viven en un subdir reservado: instalar A y "A...bak-modsync" coexiste sin chocar.
        let base = std::env::temp_dir().join("sts2_modsync_commit_nocollide");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let dest_a = base.join("a.dll");
        let dest_b = base.join("a.dll.bak-modsync"); // nombre que antes colisionaba
        std::fs::write(&dest_a, b"viejo A").unwrap(); // ambos con un viejo presente
        std::fs::write(&dest_b, b"viejo B").unwrap();
        std::fs::write(part_path(&dest_a), b"nuevo A").unwrap();
        std::fs::write(part_path(&dest_b), b"nuevo B").unwrap();
        let staged = vec![
            (part_path(&dest_a), dest_a.clone(), 0usize),
            (part_path(&dest_b), dest_b.clone(), 0usize),
        ];
        commit_staged(&staged, &base).unwrap();
        assert_eq!(std::fs::read(&dest_a).unwrap(), b"nuevo A");
        assert_eq!(std::fs::read(&dest_b).unwrap(), b"nuevo B"); // NO se perdio
        assert!(!base.join(BACKUP_DIR).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn commit_staged_no_pisa_un_backup_preservado_de_un_run_previo() {
        // Simula un commit previo cuyo rollback fallo y dejo el ORIGINAL del usuario en bak-0.
        // Un commit nuevo arranca el contador en 0 pero NO debe reusar bak-0 y pisarlo.
        let base = std::env::temp_dir().join("sts2_modsync_commit_preserved");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join(BACKUP_DIR)).unwrap();
        let preserved = base.join(BACKUP_DIR).join("bak-0");
        std::fs::write(&preserved, b"ORIGINAL del usuario").unwrap();

        let dest = base.join("a.dll");
        std::fs::write(&dest, b"viejo").unwrap(); // hay un viejo -> se respalda en un bak libre
        std::fs::write(part_path(&dest), b"nuevo").unwrap();
        let staged = vec![(part_path(&dest), dest.clone(), 0usize)];
        commit_staged(&staged, &base).unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"nuevo");
        // el backup preservado del run previo sigue INTACTO (no se piso con bak-0).
        assert_eq!(std::fs::read(&preserved).unwrap(), b"ORIGINAL del usuario");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn plan_no_marca_los_part_como_huerfanos() {
        let base = std::env::temp_dir().join("sts2_modsync_plan_part");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Mod")).unwrap();
        let manifest = manifest_one("Mod", "Mod/a.txt");
        std::fs::write(base.join("Mod").join("a.txt"), content_for("Mod/a.txt")).unwrap();
        std::fs::write(base.join("Mod").join("a.txt.part"), b"a medias").unwrap();
        std::fs::write(base.join("Mod").join("viejo.txt"), b"huerfano").unwrap();
        let plan = plan(&manifest, &base).unwrap();
        assert!(plan.up_to_date.contains(&"Mod/a.txt".to_string()));
        let orphans: Vec<String> = plan
            .orphans
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        assert!(
            orphans.iter().any(|o| o.ends_with("viejo.txt")),
            "viejo.txt deberia ser huerfano"
        );
        assert!(
            !orphans.iter().any(|o| o.ends_with(".part")),
            "el .part NO debe ser huerfano: {orphans:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(windows)]
    #[test]
    fn plan_no_manda_a_la_papelera_un_archivo_que_solo_difiere_en_mayusculas() {
        // En Windows (FS case-insensitive) el archivo en disco `Mod/BaseLib.pck` ES el que el
        // manifest pide como `Mod/baselib.pck`: NO debe quedar como huerfano (antes lo trasheaba).
        let base = std::env::temp_dir().join("sts2_modsync_plan_casing");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Mod")).unwrap();
        let content = content_for("Mod/baselib.pck");
        std::fs::write(base.join("Mod").join("BaseLib.pck"), &content).unwrap();
        let manifest = manifest_one("Mod", "Mod/baselib.pck");

        let plan = plan(&manifest, &base).unwrap();
        let orphans: Vec<String> = plan
            .orphans
            .iter()
            .map(|p| p.display().to_string().to_lowercase())
            .collect();
        assert!(
            !orphans.iter().any(|o| o.ends_with("baselib.pck")),
            "el archivo que solo difiere en mayusculas NO debe ser huerfano: {orphans:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn long_path_solo_actua_en_rutas_largas() {
        let short = std::env::temp_dir().join("x");
        assert_eq!(long_path(&short).as_ref(), short.as_path()); // identidad en lo comun
        #[cfg(windows)]
        {
            let long = "d".repeat(300);
            // ruta larga con letra de unidad -> prefijo verbatim.
            let drive = PathBuf::from(format!("C:\\{long}\\f.dll"));
            assert!(
                long_path(&drive)
                    .to_string_lossy()
                    .starts_with("\\\\?\\C:\\")
            );
            // ruta UNC larga -> \\?\UNC\server\share\... (NO el \\?\\\server malformado).
            let unc = PathBuf::from(format!("\\\\server\\share\\{long}\\f.dll"));
            let got = long_path(&unc).to_string_lossy().into_owned();
            assert!(
                got.starts_with("\\\\?\\UNC\\server\\share\\"),
                "UNC mal armada: {got}"
            );
            // ya-prefijada larga -> identidad (no duplicar el prefijo).
            let pre = PathBuf::from(format!("\\\\?\\C:\\{long}\\f.dll"));
            assert_eq!(long_path(&pre).as_ref(), pre.as_path());
            // relativa larga -> identidad (\\?\ exige ruta absoluta).
            let rel = PathBuf::from(format!("{long}\\f.dll"));
            assert_eq!(long_path(&rel).as_ref(), rel.as_path());
        }
    }

    /// Crea una carpeta de mod `<base>/<folder>` con un `<folder>.json` que declara `id`.
    fn mk_mod(base: &Path, folder: &str, id: &str) {
        let dir = base.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{folder}.json")),
            format!(r#"{{"id":"{id}"}}"#),
        )
        .unwrap();
    }

    /// `Install` de prueba con `mods/` (y `mods_disabled/` hermano) bajo `base`.
    fn fake_install(base: &Path) -> Install {
        Install {
            root: base.to_path_buf(),
            mods_dir: base.join("mods"),
            version: None,
            source: crate::detect::Source::Manual,
        }
    }

    #[test]
    fn duplicate_folders_to_clean_encuentra_copias_con_otro_nombre() {
        let base = std::env::temp_dir().join("sts2_modsync_dupfolders");
        let _ = std::fs::remove_dir_all(&base);
        let mods = base.join("mods");
        let disabled = base.join(crate::modlist::DISABLED_DIRNAME);
        mk_mod(&mods, "Mod", "Mod"); // canonica (keeper)
        mk_mod(&mods, "Mod-v2", "Mod"); // copia habilitada con otro nombre -> limpiar
        mk_mod(&disabled, "Mod-bak", "Mod"); // copia deshabilitada -> limpiar
        mk_mod(&mods, "Otro", "Otro"); // mod ajeno NO gestionado -> intacto

        let manifest = manifest_one("Mod", "Mod/a.txt");
        let dups = duplicate_folders_to_clean(&manifest, &fake_install(&base));
        let names: BTreeSet<String> = dups
            .iter()
            .filter_map(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .collect();
        assert!(names.contains("Mod-v2"), "falta la copia con otro nombre");
        assert!(names.contains("Mod-bak"), "falta la copia deshabilitada");
        assert!(
            !names.contains("Mod"),
            "NO debe limpiar la carpeta canonica"
        );
        assert!(!names.contains("Otro"), "NO debe tocar un mod ajeno");
        assert_eq!(dups.len(), 2);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn duplicate_folders_to_clean_no_borra_la_unica_copia() {
        // Si NO existe la canonica `mods/<id>/`, una copia con otro nombre es la UNICA -> no tocarla
        // (no dejar al usuario sin el mod).
        let base = std::env::temp_dir().join("sts2_modsync_dupfolders_solo");
        let _ = std::fs::remove_dir_all(&base);
        let mods = base.join("mods");
        mk_mod(&mods, "Mod-v2", "Mod"); // unica copia, con otro nombre, SIN canonica
        let manifest = manifest_one("Mod", "Mod/a.txt");
        assert!(duplicate_folders_to_clean(&manifest, &fake_install(&base)).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_falla_si_el_hash_no_coincide_y_no_escribe_destino() {
        let base = std::env::temp_dir().join("sts2_modsync_apply_bad");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let manifest = manifest_one("Mod", "Mod/a.txt");
        let plan = plan(&manifest, &base).unwrap();

        let err = apply(
            &plan,
            &manifest,
            &base,
            &BadSource,
            &mut |_| {},
            &mut |_| {},
            &AtomicBool::new(false),
        );
        assert!(err.is_err());
        // el destino NO se creo (solo habia un .part, que se borro al fallar la verificacion).
        assert!(!base.join("Mod").join("a.txt").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
