//! Planificador de sincronizacion: compara el estado real de `mods/` contra el
//! set-manifest y produce un PLAN (que bajar, que ya esta al dia, que sobra).
//! El APPLY (descarga transaccional + verificacion + rename atomico) es FASE 2;
//! aqui queda su contrato y un dry-run honesto y seguro.

use crate::hashing;
use crate::manifest::{FileEntry, SetManifest};
use anyhow::Result;
use std::collections::BTreeSet;
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

/// Aplica el plan. FASE 2 (siguiente Claude Code) — ver HANDOFF.md §sync:
///  1. abortar si `detect::is_game_running()` (lock de .dll/.pck en Windows);
///  2. por cada `to_download`, bajar con `transport::ModSource` a `<dest>.part`
///     (reanudable via HTTP Range) y verificar blake3 contra el manifiesto;
///  3. SOLO si TODO verifico, renombrar `.part` -> destino de forma atomica
///     (tempfile/persist en el MISMO volumen) en orden topologico — transaccion
///     all-or-nothing (si algo falla, no se renombra nada);
///  4. borrar huerfanos (con backup `.bak` o papelera) tras el exito.
pub fn apply(_plan: &Plan, _manifest: &SetManifest, _mods_dir: &Path) -> Result<()> {
    anyhow::bail!("apply() es FASE 2 (transporte + escritura transaccional) — ver HANDOFF.md")
}
