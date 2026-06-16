//! Mod manager — modelo + escaneo de los mods INSTALADOS en `<StS2>/mods` (habilitados)
//! y `<StS2>/mods_disabled` (deshabilitados). El artefacto que se lee aca es el
//! `<id>.json` que cada mod trae PARA EL JUEGO (`ModManifest`) — NO confundir con el
//! `manifest::SetManifest` (artefacto de la sync). Este modulo es **solo-lectura**; las
//! mutaciones (enable/disable/install/uninstall) viven en `manager`.

use crate::detect::Install;
use crate::manifest::{LOAD_ORDER_ENFORCER_ID, canonical_order};
use anyhow::Result;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Directorio hermano de `mods/` donde van los mods DESHABILITADOS (el juego NO lo
/// escanea). Definido aca y reusado por `manager`.
pub const DISABLED_DIRNAME: &str = "mods_disabled";

/// El `<id>.json` (o legacy `mod_manifest.json`) que un mod trae para el juego. Campos
/// laxos: los mods reales varian, asi que todo menos `id` es opcional / con default.
#[derive(Debug, Clone, Deserialize)]
pub struct ModManifest {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub has_dll: bool,
    #[serde(default)]
    pub has_pck: bool,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub affects_gameplay: bool,
    /// Origen del mod (repo de GitHub o pagina de Nexus), si el modder lo declara en el `<id>.json`.
    /// Se prueban en orden; el primero que parsee a un `ModSource` gana. Opcional (pocos lo traen
    /// hoy): si falta, el usuario lo pega a mano y se recuerda en `config.mod_sources`.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
}

impl ModManifest {
    /// Nombre legible (cae al id si el manifiesto no trae `name`).
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// Origen declarado en el `<id>.json` (si alguno de `repository`/`url`/`homepage` parsea a un
    /// `ModSource`). El override del usuario en `config.mod_sources` tiene prioridad sobre esto.
    pub fn source_hint(&self) -> Option<crate::modsource::ModSource> {
        [&self.repository, &self.url, &self.homepage]
            .into_iter()
            .flatten()
            .find_map(|s| crate::modsource::ModSource::parse(s))
    }
}

/// Un mod instalado en disco (habilitado o no).
#[derive(Debug, Clone)]
pub struct InstalledMod {
    pub manifest: ModManifest,
    /// Carpeta del mod (`mods/<id>` o `mods_disabled/<id>`).
    pub dir: PathBuf,
    pub enabled: bool,
    pub size_bytes: u64,
}

impl InstalledMod {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }
}

#[cfg(test)]
impl InstalledMod {
    /// Constructor de prueba: un mod con solo `id` + estado `enabled`, sin tocar disco.
    pub fn fake(id: &str, enabled: bool) -> Self {
        InstalledMod {
            manifest: serde_json::from_str(&format!(r#"{{"id":"{id}"}}"#)).unwrap(),
            dir: PathBuf::from(id),
            enabled,
            size_bytes: 0,
        }
    }
}

/// `<root>/mods_disabled` (hermano de `mods/`).
pub fn disabled_dir(install: &Install) -> PathBuf {
    install
        .mods_dir
        .parent()
        .unwrap_or(&install.mods_dir)
        .join(DISABLED_DIRNAME)
}

/// Escanea mods habilitados (`mods/`) + deshabilitados (`mods_disabled/`). Ignora
/// carpetas sin un manifiesto parseable. Ordena por nombre legible (case-insensitive).
pub fn scan(install: &Install) -> Result<Vec<InstalledMod>> {
    let mut mods = Vec::new();
    scan_dir(&install.mods_dir, true, &mut mods);
    scan_dir(&disabled_dir(install), false, &mut mods);
    mods.sort_by_key(|m| m.manifest.display_name().to_ascii_lowercase());
    Ok(mods)
}

fn scan_dir(dir: &Path, enabled: bool, out: &mut Vec<InstalledMod>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(manifest) = read_manifest(&path) {
            out.push(InstalledMod {
                manifest,
                size_bytes: dir_size(&path),
                dir: path,
                enabled,
            });
        }
    }
}

/// Busca el manifiesto del mod en su carpeta: `<carpeta>.json` -> `mod_manifest.json`
/// (legacy) -> primer `*.json` que parsee como `ModManifest`. `None` si no hay ninguno
/// (la carpeta no es un mod). Reusado por `manager` al instalar.
pub fn read_manifest(mod_dir: &Path) -> Option<ModManifest> {
    let folder = mod_dir.file_name()?.to_string_lossy().to_string();
    let mut candidates: Vec<PathBuf> = vec![
        mod_dir.join(format!("{folder}.json")),
        mod_dir.join("mod_manifest.json"),
    ];
    if let Ok(rd) = std::fs::read_dir(mod_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                candidates.push(p);
            }
        }
    }
    for c in candidates {
        // trim_start: varios `<id>.json` (p.ej. BaseLib) vienen con BOM UTF-8, que rompe
        // serde_json. Lo sacamos antes de parsear.
        if let Ok(txt) = std::fs::read_to_string(&c)
            && let Ok(m) = serde_json::from_str::<ModManifest>(txt.trim_start_matches('\u{feff}'))
        {
            return Some(m);
        }
    }
    None
}

fn dir_size(dir: &Path) -> u64 {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Carpetas DIRECTAS de `mods/` y `mods_disabled/` que tienen un `<id>.json` parseable, como pares
/// (carpeta, id_declarado). Liviano (NO calcula tamaños como `scan`). Es la base comun para atribuir
/// una carpeta a un id por su MANIFIESTO (no por el nombre) — la usan `manager::dirs_with_id` (que
/// copias de un id reemplazar) y `sync::duplicate_folders_to_clean` (que copias duplicadas limpiar):
/// un solo escaneo del area gestionada, con el MISMO criterio de atribucion para no divergir.
/// LIMITE inherente (igual que antes): una carpeta con manifiesto ilegible no se puede atribuir.
pub fn folders_with_declared_id(install: &Install) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for base in [install.mods_dir.clone(), disabled_dir(install)] {
        let Ok(rd) = std::fs::read_dir(&base) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir()
                && let Some(m) = read_manifest(&p)
            {
                out.push((p, m.id));
            }
        }
    }
    out
}

/// Pares (mod, dependencia) donde la dependencia no esta presente y HABILITADA. Solo
/// para mods habilitados (un mod deshabilitado no carga; su dep no importa).
pub fn missing_dependencies(mods: &[InstalledMod]) -> Vec<(String, String)> {
    let enabled: BTreeSet<&str> = mods.iter().filter(|m| m.enabled).map(|m| m.id()).collect();
    let mut out = Vec::new();
    for m in mods.iter().filter(|m| m.enabled) {
        for dep in &m.manifest.dependencies {
            if !enabled.contains(dep.as_str()) {
                out.push((m.id().to_string(), dep.clone()));
            }
        }
    }
    out
}

/// Ids de dependencias FALTANTES que estan instaladas pero DESHABILITADAS: se pueden habilitar
/// con un clic (a diferencia de las que no estan instaladas, que hay que bajar). Deduplicado.
pub fn enableable_missing_deps(mods: &[InstalledMod]) -> Vec<String> {
    let present_disabled: BTreeSet<&str> =
        mods.iter().filter(|m| !m.enabled).map(|m| m.id()).collect();
    let mut out: BTreeSet<String> = BTreeSet::new();
    for (_, dep) in missing_dependencies(mods) {
        if present_disabled.contains(dep.as_str()) {
            out.insert(dep);
        }
    }
    out.into_iter().collect()
}

/// Ids que aparecen mas de una vez (p.ej. la misma carpeta en `mods/` y `mods_disabled/`,
/// o dos mods declarando el mismo id). Util como warning de conflicto.
pub fn conflicts(mods: &[InstalledMod]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut dup = BTreeSet::new();
    for m in mods {
        if !seen.insert(m.id()) {
            dup.insert(m.id().to_string());
        }
    }
    dup.into_iter().collect()
}

/// Un grupo de mods DUPLICADOS: el MISMO id instalado en mas de una carpeta. `keep` es el que se
/// conserva y `remove` los demas (a mandar a la papelera). El que se conserva es la version MAS
/// NUEVA (asi "si hay 2 versiones del mismo mod, se borra la vieja").
#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub id: String,
    pub keep: InstalledMod,
    pub remove: Vec<InstalledMod>,
}

/// Detecta ids instalados en MAS DE UNA carpeta (duplicado/conflicto) y, por grupo, decide cual
/// conservar y cuales borrar. Vacio si no hay duplicados. Se conserva el de mayor `version`
/// (`update::parse_ver`); en empate, el habilitado y luego el de carpeta modificada mas
/// recientemente. Cada grupo tiene >=1 mod en `remove`.
pub fn duplicates(mods: &[InstalledMod]) -> Vec<DuplicateGroup> {
    use std::collections::BTreeMap;
    let mut by_id: BTreeMap<&str, Vec<&InstalledMod>> = BTreeMap::new();
    for m in mods {
        by_id.entry(m.id()).or_default().push(m);
    }
    let mut out = Vec::new();
    for (id, mut members) in by_id {
        if members.len() < 2 {
            continue;
        }
        // De "mejor a conservar" a "peor" (descendente): version, habilitado, mtime.
        members.sort_by_key(|m| std::cmp::Reverse(keep_rank(m)));
        let keep = members[0].clone();
        let remove = members[1..].iter().map(|&m| m.clone()).collect();
        out.push(DuplicateGroup {
            id: id.to_string(),
            keep,
            remove,
        });
    }
    out
}

/// Clave de "que tan bueno es conservar este mod" (mas alto = se conserva): (version, NO-prerelease,
/// habilitado, mtime de la carpeta). La version manda; un release (`1.2.0`) gana a un pre-release del
/// MISMO X.Y.Z (`1.2.0-beta`); el resto rompe empates entre copias de la misma version.
fn keep_rank(m: &InstalledMod) -> ((u64, u64, u64), bool, bool, u64) {
    let v = m.manifest.version.as_deref().unwrap_or("");
    let ver = crate::update::parse_ver(v);
    // 1.2.0 > 1.2.0-beta para el mismo X.Y.Z. Misma definicion de "prerelease" que el auto-update
    // (`crate::update::is_prerelease`) para que dedup y update no difieran.
    let is_release = !crate::update::is_prerelease(v);
    let mtime = std::fs::metadata(&m.dir)
        .and_then(|md| md.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (ver, is_release, m.enabled, mtime)
}

/// Orden de carga canonico (BaseLib + A-Z) sobre los mods HABILITADOS — lo que alimenta
/// el room-hash de multiplayer (reusa `manifest::canonical_order`).
pub fn load_order(mods: &[InstalledMod]) -> Vec<String> {
    canonical_order(
        mods.iter()
            .filter(|m| m.enabled)
            .map(|m| m.id().to_string()),
    )
}

/// True si `ModListSorter` esta presente y HABILITADO (el enforcer del orden). Sin el, el
/// orden de carga puede divergir entre amigos -> no entran al lobby (room-hash distinto).
pub fn load_order_enforced(mods: &[InstalledMod]) -> bool {
    mods.iter()
        .any(|m| m.enabled && m.id() == LOAD_ORDER_ENFORCER_ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn im(id: &str, enabled: bool, deps: &[&str]) -> InstalledMod {
        InstalledMod {
            manifest: ModManifest {
                id: id.into(),
                name: None,
                author: None,
                description: None,
                version: None,
                has_dll: false,
                has_pck: false,
                dependencies: deps.iter().map(|s| s.to_string()).collect(),
                affects_gameplay: false,
                url: None,
                homepage: None,
                repository: None,
            },
            dir: PathBuf::from(id),
            enabled,
            size_bytes: 0,
        }
    }

    #[test]
    fn missing_deps_solo_cuenta_habilitados() {
        let mods = vec![
            im("FGOCore", true, &["BaseLib"]),
            im("BaseLib", false, &[]), // presente pero DESHABILITADO -> sigue faltando
            im("Solo", true, &["NoExiste"]),
        ];
        let missing = missing_dependencies(&mods);
        assert!(missing.contains(&("FGOCore".into(), "BaseLib".into())));
        assert!(missing.contains(&("Solo".into(), "NoExiste".into())));
    }

    #[test]
    fn enableable_missing_deps_solo_los_instalados_deshabilitados() {
        let mods = vec![
            im("FGOCore", true, &["BaseLib", "NoExiste"]),
            im("BaseLib", false, &[]), // instalado pero deshabilitado -> habilitable
        ];
        // BaseLib se puede habilitar; "NoExiste" no esta instalado -> no aparece.
        assert_eq!(enableable_missing_deps(&mods), vec!["BaseLib".to_string()]);
    }

    #[test]
    fn load_order_canonico_solo_habilitados() {
        let mods = vec![
            im("FGOCore", true, &[]),
            im("BaseLib", true, &[]),
            im("Acheron", false, &[]), // deshabilitado -> no entra al orden
            im("ModListSorter", true, &[]),
        ];
        assert_eq!(
            load_order(&mods),
            ["BaseLib", "FGOCore", "ModListSorter"]
                .map(String::from)
                .to_vec()
        );
        assert!(load_order_enforced(&mods));
        let sin = vec![im("BaseLib", true, &[])];
        assert!(!load_order_enforced(&sin));
    }

    #[test]
    fn duplicates_conserva_la_version_mas_nueva() {
        // Mismo id en dos carpetas distintas, distinta version: se conserva la nueva, se borra la vieja.
        let mut viejo = im("FGOCore", true, &[]);
        viejo.manifest.version = Some("1.0.0".into());
        viejo.dir = PathBuf::from("mods/FGOCore");
        let mut nuevo = im("FGOCore", false, &[]);
        nuevo.manifest.version = Some("1.2.0".into());
        nuevo.dir = PathBuf::from("mods/FGOCore-1.2");
        let mods = vec![viejo, nuevo, im("Solo", true, &[])];

        let dups = duplicates(&mods);
        assert_eq!(dups.len(), 1, "solo FGOCore esta duplicado");
        let g = &dups[0];
        assert_eq!(g.id, "FGOCore");
        assert_eq!(
            g.keep.manifest.version.as_deref(),
            Some("1.2.0"),
            "conserva la version mas nueva"
        );
        assert_eq!(g.remove.len(), 1);
        assert_eq!(
            g.remove[0].manifest.version.as_deref(),
            Some("1.0.0"),
            "borra la vieja"
        );
        // Sin duplicados -> vacio.
        assert!(duplicates(&[im("A", true, &[]), im("B", false, &[])]).is_empty());
    }

    #[test]
    fn scan_lee_habilitados_y_deshabilitados() {
        let base = std::env::temp_dir().join("sts2_modsync_scan_test");
        let _ = std::fs::remove_dir_all(&base);
        let mods_dir = base.join("mods");
        let disabled = base.join(DISABLED_DIRNAME);
        // mod habilitado con <id>.json — CON BOM UTF-8 (como el BaseLib real), para
        // cubrir que el scan lo tolere.
        std::fs::create_dir_all(mods_dir.join("BaseLib")).unwrap();
        std::fs::write(
            mods_dir.join("BaseLib").join("BaseLib.json"),
            "\u{feff}".to_string() + r#"{"id":"BaseLib","name":"BaseLib","has_dll":true}"#,
        )
        .unwrap();
        // mod deshabilitado con mod_manifest.json (legacy)
        std::fs::create_dir_all(disabled.join("OldMod")).unwrap();
        std::fs::write(
            disabled.join("OldMod").join("mod_manifest.json"),
            r#"{"id":"OldMod"}"#,
        )
        .unwrap();
        // carpeta sin manifiesto -> se ignora
        std::fs::create_dir_all(mods_dir.join("NoManifest")).unwrap();

        let install = Install {
            root: base.clone(),
            mods_dir,
            version: None,
            source: crate::detect::Source::Manual,
        };
        let mut mods = scan(&install).unwrap();
        mods.sort_by(|a, b| a.id().cmp(b.id()));
        assert_eq!(mods.len(), 2);
        let baselib = mods.iter().find(|m| m.id() == "BaseLib").unwrap();
        assert!(baselib.enabled && baselib.manifest.has_dll);
        let old = mods.iter().find(|m| m.id() == "OldMod").unwrap();
        assert!(!old.enabled);

        let _ = std::fs::remove_dir_all(&base);
    }
}
