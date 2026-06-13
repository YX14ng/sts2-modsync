//! Formato del "set-manifest": describe EXACTAMENTE que mods gestiona un set y,
//! por mod, sus archivos con hash y tamano. Es un artefacto NUEVO — NO confundir
//! con el `<Id>.json` que cada mod tiene para el juego.
//!
//! Reglas de oro (ver HANDOFF.md §seguridad):
//!  - El conjunto de `ModEntry.id` define las CARPETAS que el sync puede tocar
//!    (`<StS2>/mods/<id>/`). Cualquier carpeta no listada es intocable.
//!  - `files[].path` es relativa a `<StS2>/mods/` y DEBE quedar contenida dentro
//!    de alguna `<id>/` listada (sin `..`, sin rutas absolutas) — `validate_paths`.
//!  - `dependencies` permite instalar en orden topologico (BaseLib -> FGOCore ->
//!    personajes) para no dejar un personaje contra una libreria a medio bajar.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Version del esquema del manifiesto (subir si cambia la forma).
pub const SCHEMA_VERSION: u32 = 1;

/// Libreria base: se carga SIEMPRE primero (la fija arriba BaseLib/ModListSorter).
pub const BASELIB_ID: &str = "BaseLib";
/// Mod que fuerza el orden de carga canonico (BaseLib + A-Z) al cerrar el juego. Debe
/// estar en el set para que todos los amigos converjan al mismo orden -> mismo room-hash.
pub const LOAD_ORDER_ENFORCER_ID: &str = "ModListSorter";

/// Orden de carga canonico (BaseLib primero, el resto A-Z case-insensitive) sobre una
/// coleccion de ids. Compartido por el set-manifest (sync) y el mod manager (`modlist`):
/// es el orden que fuerza `ModListSorter` en runtime y alimenta el room-hash de BaseLib.
pub fn canonical_order(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut ids: Vec<String> = ids.into_iter().collect();
    ids.sort_by_key(|id| (id != BASELIB_ID, id.to_ascii_lowercase()));
    ids
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetManifest {
    /// Version del esquema; debe ser <= SCHEMA_VERSION.
    pub schema: u32,
    /// Nombre legible del set (p.ej. "FGO de Chaldea").
    pub set_name: String,
    /// Version del set (semver o fecha); cambia en cada publicacion.
    pub set_version: String,
    /// Marca de tiempo ISO-8601 de publicacion.
    pub published_at: String,
    /// id de la clave de firma usada (para rotacion); None si el set no esta firmado.
    #[serde(default)]
    pub signing_key_id: Option<String>,
    /// Base desde donde se descargan los archivos (p.ej. la URL de un GitHub Release).
    /// Cada `FileEntry.path` se resuelve relativo a esta base.
    pub base_url: String,
    /// Version de BaseLib con la que se compilaron estos mods (pin); el cliente
    /// avisa si el install tiene otra (ReflectionTypeLoadException si difieren).
    #[serde(default)]
    pub baselib_version: Option<String>,
    /// Los mods del set, en cualquier orden (el orden de instalacion se calcula
    /// topologicamente con `install_order`).
    pub mods: Vec<ModEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEntry {
    /// Id del mod = nombre de la carpeta gestionada bajo `mods/` (p.ej. "MashShielder").
    pub id: String,
    /// Version del mod.
    pub version: String,
    /// Ids de los mods de los que depende (deben estar en el mismo set).
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Archivos del mod, con su hash y tamano.
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// Ruta relativa a `<StS2>/mods/`, separada por `/` (p.ej. "MashShielder/MashShielder.dll").
    pub path: String,
    /// Tamano en bytes (para barra de progreso y verificacion previa).
    pub size: u64,
    /// Hash BLAKE3 en hex del contenido.
    pub blake3: String,
}

impl SetManifest {
    pub fn from_json_str(s: &str) -> Result<Self> {
        // Saca un BOM UTF-8 si lo trae (editores en Windows lo agregan; rompe serde_json).
        let s = s.trim_start_matches('\u{feff}');
        let m: SetManifest = serde_json::from_str(s).context("manifiesto JSON invalido")?;
        m.validate()?;
        Ok(m)
    }

    pub fn from_json_file(path: &Path) -> Result<Self> {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("no se pudo leer el manifiesto {}", path.display()))?;
        Self::from_json_str(&s)
    }

    /// Validaciones estructurales + de seguridad (paths). Llamada en cada `from_*`.
    pub fn validate(&self) -> Result<()> {
        if self.schema > SCHEMA_VERSION {
            bail!(
                "el manifiesto usa schema {} > {} soportado — actualiza la app",
                self.schema,
                SCHEMA_VERSION
            );
        }
        self.validate_paths()?;
        self.validate_dependencies()?;
        Ok(())
    }

    /// Cada `files[].path` debe quedar contenido dentro de `<id>/` de un mod LISTADO,
    /// sin `..`, sin raiz absoluta, sin separadores ambiguos. Cierra el path-traversal.
    pub fn validate_paths(&self) -> Result<()> {
        let ids: BTreeSet<&str> = self.mods.iter().map(|m| m.id.as_str()).collect();
        for m in &self.mods {
            for f in &m.files {
                let p = &f.path;
                if p.is_empty()
                    || p.starts_with('/')
                    || p.starts_with('\\')
                    || p.contains(':')
                    || p.split(['/', '\\']).any(|seg| seg == ".." || seg == ".")
                {
                    bail!("path inseguro en el manifiesto: {:?}", p);
                }
                // El primer segmento (la carpeta del mod) debe ser un id listado, y
                // coincidir con el mod que declara el archivo.
                let first = p.split(['/', '\\']).next().unwrap_or("");
                if first != m.id {
                    bail!(
                        "el archivo {:?} no esta bajo la carpeta de su mod {:?}",
                        p,
                        m.id
                    );
                }
                if !ids.contains(first) {
                    bail!("path {:?} fuera de las carpetas gestionadas", p);
                }
            }
        }
        Ok(())
    }

    /// Toda dependencia declarada debe existir como mod del set.
    fn validate_dependencies(&self) -> Result<()> {
        let ids: BTreeSet<&str> = self.mods.iter().map(|m| m.id.as_str()).collect();
        for m in &self.mods {
            for d in &m.dependencies {
                if !ids.contains(d.as_str()) {
                    bail!(
                        "el mod {:?} depende de {:?}, que no esta en el set",
                        m.id,
                        d
                    );
                }
            }
        }
        Ok(())
    }

    /// Carpetas (ids) que el sync puede crear/actualizar/limpiar. Todo lo demas en
    /// `mods/` es intocable.
    pub fn managed_ids(&self) -> BTreeSet<String> {
        self.mods.iter().map(|m| m.id.clone()).collect()
    }

    /// Orden de carga CANONICO para el room-hash de BaseLib en multiplayer: BaseLib
    /// primero, el resto alfabetico A-Z (case-insensitive), igual que fuerza
    /// `ModListSorter` al cerrar el juego. OJO: distinto de `install_order` (toposort de
    /// dependencias) — no confundirlos. ModListSorter es la autoridad real en runtime;
    /// esto es la vista/garantia del lado del sync (lo que vera multiplayer).
    pub fn canonical_load_order(&self) -> Vec<String> {
        canonical_order(self.managed_ids())
    }

    /// True si el set incluye el enforcer de orden (`ModListSorter`). Sin el en el set,
    /// los amigos pueden quedar con otro orden -> room-hash distinto -> no entran al lobby.
    pub fn syncs_load_order(&self) -> bool {
        self.managed_ids().contains(LOAD_ORDER_ENFORCER_ID)
    }

    /// Orden de instalacion topologico (dependencias primero). Error si hay ciclo.
    pub fn install_order(&self) -> Result<Vec<String>> {
        let mut deps: BTreeMap<&str, &[String]> = BTreeMap::new();
        for m in &self.mods {
            deps.insert(m.id.as_str(), &m.dependencies);
        }
        let mut order = Vec::new();
        let mut done: BTreeSet<String> = BTreeSet::new();
        // Estados: 0 = sin visitar, 1 = en pila (detecta ciclo), 2 = listo.
        let mut state: BTreeMap<&str, u8> = deps.keys().map(|k| (*k, 0u8)).collect();

        fn visit<'a>(
            id: &'a str,
            deps: &BTreeMap<&'a str, &'a [String]>,
            state: &mut BTreeMap<&'a str, u8>,
            done: &mut BTreeSet<String>,
            order: &mut Vec<String>,
        ) -> Result<()> {
            match state.get(id).copied().unwrap_or(2) {
                2 => return Ok(()),
                1 => bail!("ciclo de dependencias en torno a {:?}", id),
                _ => {}
            }
            state.insert(id, 1);
            if let Some(ds) = deps.get(id) {
                for d in ds.iter() {
                    visit(d.as_str(), deps, state, done, order)?;
                }
            }
            state.insert(id, 2);
            if done.insert(id.to_string()) {
                order.push(id.to_string());
            }
            Ok(())
        }

        let ids: Vec<&str> = deps.keys().copied().collect();
        for id in ids {
            visit(id, &deps, &mut state, &mut done, &mut order)?;
        }
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mod_with(id: &str, deps: &[&str], paths: &[&str]) -> ModEntry {
        ModEntry {
            id: id.into(),
            version: "1".into(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            files: paths
                .iter()
                .map(|p| FileEntry {
                    path: (*p).into(),
                    size: 1,
                    blake3: "00".into(),
                })
                .collect(),
        }
    }

    fn manifest(mods: Vec<ModEntry>) -> SetManifest {
        SetManifest {
            schema: 1,
            set_name: "t".into(),
            set_version: "1".into(),
            published_at: "now".into(),
            signing_key_id: None,
            base_url: "https://x/".into(),
            baselib_version: None,
            mods,
        }
    }

    #[test]
    fn install_order_pone_dependencias_primero() {
        let m = manifest(vec![
            mod_with(
                "MashShielder",
                &["BaseLib", "FGOCore"],
                &["MashShielder/a.dll"],
            ),
            mod_with("FGOCore", &["BaseLib"], &["FGOCore/a.dll"]),
            mod_with("BaseLib", &[], &["BaseLib/a.dll"]),
        ]);
        let order = m.install_order().unwrap();
        let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
        assert!(pos("BaseLib") < pos("FGOCore"));
        assert!(pos("FGOCore") < pos("MashShielder"));
    }

    #[test]
    fn install_order_detecta_ciclo() {
        let m = manifest(vec![
            mod_with("A", &["B"], &["A/x"]),
            mod_with("B", &["A"], &["B/x"]),
        ]);
        assert!(m.install_order().is_err());
    }

    #[test]
    fn validate_paths_rechaza_traversal_y_absolutas() {
        for bad in [
            "BaseLib/../evil.dll",
            "/etc/passwd",
            "C:\\win\\x.dll",
            "Otro/x.dll",
        ] {
            let m = manifest(vec![mod_with("BaseLib", &[], &[bad])]);
            assert!(m.validate_paths().is_err(), "deberia rechazar {bad:?}");
        }
    }

    #[test]
    fn validate_paths_acepta_ruta_buena() {
        let m = manifest(vec![mod_with("BaseLib", &[], &["BaseLib/BaseLib.dll"])]);
        assert!(m.validate_paths().is_ok());
    }

    #[test]
    fn dependencia_inexistente_es_error() {
        let m = manifest(vec![mod_with("A", &["NoExiste"], &["A/x"])]);
        assert!(m.validate().is_err());
    }

    #[test]
    fn canonical_load_order_pone_baselib_primero_y_resto_az() {
        let m = manifest(vec![
            mod_with("FGOCore", &[], &["FGOCore/a.dll"]),
            mod_with("BaseLib", &[], &["BaseLib/a.dll"]),
            mod_with("ModListSorter", &[], &["ModListSorter/a.dll"]),
            mod_with("Acheron", &[], &["Acheron/a.dll"]),
        ]);
        let expected: Vec<String> = ["BaseLib", "Acheron", "FGOCore", "ModListSorter"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(m.canonical_load_order(), expected);
    }

    #[test]
    fn syncs_load_order_detecta_modlistsorter() {
        let con = manifest(vec![
            mod_with("BaseLib", &[], &["BaseLib/a.dll"]),
            mod_with("ModListSorter", &[], &["ModListSorter/a.dll"]),
        ]);
        assert!(con.syncs_load_order());
        let sin = manifest(vec![mod_with("BaseLib", &[], &["BaseLib/a.dll"])]);
        assert!(!sin.syncs_load_order());
    }
}
