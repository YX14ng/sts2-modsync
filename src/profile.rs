//! Perfiles del mod manager: un perfil es un conjunto NOMBRADO de ids que deben quedar
//! HABILITADOS. Aplicar un perfil = habilitar esos ids y deshabilitar el resto (folder
//! moves via `manager`). Unifica con la sync: un set sincronizado se vuelve un perfil
//! (sus `managed_ids`). Se guardan en `%APPDATA%/sts2-modsync/.../profiles/<name>.json`.

use crate::detect::Install;
use crate::{config, manager, modlist};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// Ids de los mods que el perfil deja HABILITADOS.
    pub enabled_ids: Vec<String>,
}

impl Profile {
    /// Perfil a partir del estado actual: los mods hoy habilitados.
    pub fn from_current(name: &str, mods: &[modlist::InstalledMod]) -> Self {
        Profile {
            name: name.to_string(),
            enabled_ids: mods
                .iter()
                .filter(|m| m.enabled)
                .map(|m| m.id().to_string())
                .collect(),
        }
    }
}

/// Directorio de perfiles, junto al `config.toml`.
fn profiles_dir() -> Option<PathBuf> {
    Some(config::data_dir()?.join("profiles"))
}

fn name_is_safe(name: &str) -> bool {
    // El nombre de perfil es un nombre de archivo: mismo invariante de path-traversal que el manifest.
    crate::manifest::is_simple_segment(name)
}

pub fn save(profile: &Profile) -> Result<()> {
    if !name_is_safe(&profile.name) {
        bail!("nombre de perfil invalido: {:?}", profile.name);
    }
    let dir = profiles_dir().context("no se pudo resolver el directorio de perfiles")?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", profile.name));
    let json = serde_json::to_string_pretty(profile)?;
    std::fs::write(&path, json).with_context(|| format!("escribiendo {}", path.display()))?;
    Ok(())
}

pub fn list() -> Vec<Profile> {
    let Some(dir) = profiles_dir() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "json")
            && let Ok(txt) = std::fs::read_to_string(&p)
            && let Ok(prof) = serde_json::from_str::<Profile>(&txt)
        {
            out.push(prof);
        }
    }
    out.sort_by_key(|p| p.name.to_ascii_lowercase());
    out
}

pub fn delete(name: &str) -> Result<()> {
    if !name_is_safe(name) {
        bail!("nombre de perfil invalido: {name:?}");
    }
    let dir = profiles_dir().context("no se pudo resolver el directorio de perfiles")?;
    let path = dir.join(format!("{name}.json"));
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Resultado de aplicar un perfil (o el preview de lo que aplicaria, sin mutar nada).
#[derive(Debug, Default, Clone)]
pub struct ApplyReport {
    pub enabled: Vec<String>,
    pub disabled: Vec<String>,
    pub not_installed: Vec<String>,
}

impl ApplyReport {
    /// El preview no cambia nada (no hay que habilitar/deshabilitar nada).
    pub fn is_noop(&self) -> bool {
        self.enabled.is_empty() && self.disabled.is_empty()
    }
}

/// Calcula (SIN mutar nada) que pasaria al aplicar `profile` sobre una lista de mods YA
/// escaneada: que se habilitaria, que se deshabilitaria y que ids del perfil no estan
/// instalados. Puro: sirve para mostrar el impacto antes de confirmar (preview de un codigo).
pub fn preview_from(mods: &[modlist::InstalledMod], profile: &Profile) -> ApplyReport {
    let want: BTreeSet<&str> = profile.enabled_ids.iter().map(String::as_str).collect();
    let installed: BTreeSet<&str> = mods.iter().map(|m| m.id()).collect();
    let mut report = ApplyReport::default();

    for m in mods.iter().filter(|m| m.enabled) {
        if !want.contains(m.id()) {
            report.disabled.push(m.id().to_string());
        }
    }
    for id in &profile.enabled_ids {
        if !installed.contains(id.as_str()) {
            report.not_installed.push(id.clone());
            continue;
        }
        let already_enabled = mods.iter().any(|m| m.id() == id && m.enabled);
        if !already_enabled {
            report.enabled.push(id.clone());
        }
    }
    report
}

/// Aplica un perfil: deshabilita los mods habilitados que no esten en el perfil y habilita
/// los del perfil que esten deshabilitados. Los ids del perfil que no esten instalados se
/// reportan en `not_installed`. Exige juego cerrado (lo verifica `manager`).
pub fn apply(install: &Install, profile: &Profile) -> Result<ApplyReport> {
    let mods = modlist::scan(install)?;
    let report = preview_from(&mods, profile);
    for id in &report.disabled {
        manager::disable(install, id)?;
    }
    for id in &report.enabled {
        manager::enable(install, id)?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{Install, Source};

    fn make_mod(dir: &std::path::Path, id: &str) {
        std::fs::create_dir_all(dir.join(id)).unwrap();
        std::fs::write(
            dir.join(id).join(format!("{id}.json")),
            format!(r#"{{"id":"{id}"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn from_current_y_apply_mueven_carpetas() {
        // apply() mueve carpetas via manager, que aborta si el juego corre.
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }
        let base = std::env::temp_dir().join("sts2_modsync_profile_test");
        let _ = std::fs::remove_dir_all(&base);
        let mods_dir = base.join("mods");
        let disabled = base.join(modlist::DISABLED_DIRNAME);
        make_mod(&mods_dir, "BaseLib");
        make_mod(&mods_dir, "Extra");
        make_mod(&disabled, "Char");

        let install = Install {
            root: base.clone(),
            mods_dir: mods_dir.clone(),
            version: None,
            source: Source::Manual,
        };

        // from_current = lo habilitado hoy (BaseLib, Extra).
        let now = Profile::from_current("x", &modlist::scan(&install).unwrap()).enabled_ids;
        assert!(now.contains(&"BaseLib".to_string()) && now.contains(&"Extra".to_string()));
        assert!(!now.contains(&"Char".to_string()));

        // Aplicar perfil {BaseLib, Char}: Extra se deshabilita, Char se habilita.
        let prof = Profile {
            name: "p".into(),
            enabled_ids: vec!["BaseLib".into(), "Char".into()],
        };
        let report = apply(&install, &prof).unwrap();
        assert!(report.disabled.contains(&"Extra".to_string()));
        assert!(report.enabled.contains(&"Char".to_string()));
        assert!(mods_dir.join("BaseLib").is_dir());
        assert!(mods_dir.join("Char").is_dir());
        assert!(disabled.join("Extra").is_dir());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn preview_from_es_puro_y_coincide_con_apply() {
        // preview_from no toca el disco: calcula sobre una lista ya escaneada.
        let mods = vec![
            modlist::InstalledMod::fake("BaseLib", true),
            modlist::InstalledMod::fake("Extra", true),
            modlist::InstalledMod::fake("Char", false),
        ];
        let prof = Profile {
            name: "p".into(),
            enabled_ids: vec!["BaseLib".into(), "Char".into(), "Falta".into()],
        };
        let r = preview_from(&mods, &prof);
        assert_eq!(r.enabled, vec!["Char".to_string()]); // estaba deshabilitado
        assert_eq!(r.disabled, vec!["Extra".to_string()]); // sobra
        assert_eq!(r.not_installed, vec!["Falta".to_string()]); // no instalado
        assert!(!r.is_noop());

        // Aplicar el estado destino sobre si mismo es un no-op.
        let after = vec![
            modlist::InstalledMod::fake("BaseLib", true),
            modlist::InstalledMod::fake("Extra", false),
            modlist::InstalledMod::fake("Char", true),
        ];
        let prof2 = Profile {
            name: "p".into(),
            enabled_ids: vec!["BaseLib".into(), "Char".into()],
        };
        assert!(preview_from(&after, &prof2).is_noop());
    }
}
