//! Modo PUBLICAR (lado modder): genera un set-manifest desde los mods INSTALADOS, hasheando
//! cada archivo, y junta los assets (content-addressed por BLAKE3) listos para subir a un
//! GitHub Release. Es el inverso de `sync::plan`: el modder corre esto, sube el resultado, y
//! sus amigos lo sincronizan. Reusa `modlist` (mods instalados) + `hashing` (BLAKE3).

use crate::hashing;
use crate::manifest::{BASELIB_ID, FileEntry, LOAD_ORDER_ENFORCER_ID, ModEntry, SetManifest};
use crate::modlist::InstalledMod;
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Datos del set que el modder define al publicar.
#[derive(Debug, Clone)]
pub struct PublishParams {
    pub set_name: String,
    pub set_version: String,
    /// Base de descarga (la URL del release): `https://github.com/u/r/releases/download/<tag>/`.
    /// El `<tag>` deberia ser `set_version`.
    pub base_url: String,
    /// ISO-8601; si queda vacio se usa `set_version`.
    pub published_at: String,
    pub baselib_version: Option<String>,
}

/// Un archivo a subir (asset), nombrado por su BLAKE3 (content-addressed).
#[derive(Debug, Clone)]
pub struct Asset {
    pub blake3: String,
    pub src: PathBuf,
    pub size: u64,
}

#[derive(Debug)]
pub struct Prepared {
    pub manifest: SetManifest,
    pub assets: Vec<Asset>,
}

impl Prepared {
    /// Bytes totales de los assets unicos (lo que se sube al release).
    pub fn total_bytes(&self) -> u64 {
        self.assets.iter().map(|a| a.size).sum()
    }
}

/// Extensiones que NO se publican (no las usa el juego).
fn skip_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("pdb") | Some("part")
    )
}

/// Genera el manifest + la lista de assets (dedup por blake3) para los `ids` elegidos.
/// Hashea cada archivo (puede tardar con `.pck` grandes). Falla si algun id no esta
/// instalado, si un mod no tiene archivos, o si el manifest resultante no valida.
pub fn prepare(
    mods: &[InstalledMod],
    ids: &BTreeSet<String>,
    p: &PublishParams,
) -> Result<Prepared> {
    let by_id: BTreeMap<&str, &InstalledMod> = mods.iter().map(|m| (m.id(), m)).collect();

    let mut entries: Vec<ModEntry> = Vec::new();
    let mut assets: Vec<Asset> = Vec::new();
    let mut seen_blake: BTreeSet<String> = BTreeSet::new();

    for id in ids {
        let m = by_id
            .get(id.as_str())
            .with_context(|| format!("el mod {id:?} no esta instalado"))?;
        let mut files: Vec<FileEntry> = Vec::new();
        for entry in WalkDir::new(&m.dir).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() || skip_file(entry.path()) {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&m.dir)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            let path = format!("{id}/{rel}");
            let size = entry.metadata().map(|md| md.len()).unwrap_or(0);
            let blake3 = hashing::blake3_file(entry.path())
                .with_context(|| format!("hasheando {}", entry.path().display()))?;
            if seen_blake.insert(blake3.clone()) {
                assets.push(Asset {
                    blake3: blake3.clone(),
                    src: entry.path().to_path_buf(),
                    size,
                });
            }
            files.push(FileEntry { path, size, blake3 });
        }
        if files.is_empty() {
            bail!("el mod {id:?} no tiene archivos para publicar");
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        entries.push(ModEntry {
            id: id.clone(),
            version: m.manifest.version.clone().unwrap_or_else(|| "0".into()),
            dependencies: m.manifest.dependencies.clone(),
            files,
        });
    }

    let published_at = if p.published_at.is_empty() {
        p.set_version.clone()
    } else {
        p.published_at.clone()
    };
    let manifest = SetManifest {
        schema: crate::manifest::SCHEMA_VERSION,
        set_name: p.set_name.clone(),
        set_version: p.set_version.clone(),
        published_at,
        signing_key_id: None,
        base_url: p.base_url.clone(),
        baselib_version: p.baselib_version.clone(),
        mods: entries,
    };
    manifest
        .validate()
        .context("el manifest generado no valida")?;
    Ok(Prepared { manifest, assets })
}

/// Avisos: el set deberia incluir BaseLib + ModListSorter (orden de carga multiplayer).
pub fn warnings(ids: &BTreeSet<String>) -> Vec<String> {
    let mut w = Vec::new();
    if !ids.contains(BASELIB_ID) {
        w.push(format!("el set no incluye {BASELIB_ID} (libreria base)"));
    }
    if !ids.contains(LOAD_ORDER_ENFORCER_ID) {
        w.push(format!(
            "el set no incluye {LOAD_ORDER_ENFORCER_ID}: los amigos pueden quedar con otro orden de carga (room-hash)"
        ));
    }
    w
}

/// Escribe `out_dir/set-manifest.json` + copia cada asset a `out_dir/assets/<blake3>`.
/// Devuelve el path del manifest escrito.
pub fn write_out(prep: &Prepared, out_dir: &Path) -> Result<PathBuf> {
    let assets_dir = out_dir.join("assets");
    std::fs::create_dir_all(&assets_dir)
        .with_context(|| format!("creando {}", assets_dir.display()))?;
    for a in &prep.assets {
        let dst = assets_dir.join(&a.blake3);
        if !dst.exists() {
            std::fs::copy(&a.src, &dst)
                .with_context(|| format!("copiando asset {}", a.src.display()))?;
        }
    }
    let manifest_path = out_dir.join("set-manifest.json");
    let json = serde_json::to_string_pretty(&prep.manifest)?;
    std::fs::write(&manifest_path, &json)
        .with_context(|| format!("escribiendo {}", manifest_path.display()))?;
    // Firmar si el modder tiene clave secreta (`keygen`). Sin clave => sin firma (modo dev).
    if let Some(sk) = crate::signing::load_secret_key() {
        let sig = crate::signing::sign(&sk, json.as_bytes())?;
        let sig_path = out_dir.join("set-manifest.json.minisig");
        std::fs::write(&sig_path, sig)
            .with_context(|| format!("escribiendo {}", sig_path.display()))?;
    }
    Ok(manifest_path)
}

/// Comando sugerido para subir todo a un GitHub Release con el `gh` CLI (el tag = set_version,
/// que debe coincidir con el `<tag>` del `base_url`). Incluye el `.minisig` si se firmo.
pub fn gh_hint(set_version: &str, out_dir: &Path) -> String {
    let sig = if out_dir.join("set-manifest.json.minisig").exists() {
        " set-manifest.json.minisig"
    } else {
        ""
    };
    format!(
        "cd \"{}\" && gh release create {set_version} set-manifest.json{sig} assets/*",
        out_dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{Install, Source};
    use crate::{modlist, sync};

    fn make_mod(mods_dir: &Path, id: &str, files: &[(&str, &[u8])]) {
        let dir = mods_dir.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{id}.json")),
            format!(r#"{{"id":"{id}","version":"1.0"}}"#),
        )
        .unwrap();
        for (rel, content) in files {
            std::fs::write(dir.join(rel), content).unwrap();
        }
    }

    #[test]
    fn publish_round_trip_da_plan_noop() {
        let base = std::env::temp_dir().join("sts2_modsync_publish_test");
        let _ = std::fs::remove_dir_all(&base);
        let mods_dir = base.join("mods");
        make_mod(&mods_dir, "BaseLib", &[("BaseLib.dll", b"dll-bytes")]);
        make_mod(
            &mods_dir,
            "Char",
            &[("Char.dll", b"char-bytes"), ("data.pck", b"pck-bytes")],
        );

        let install = Install {
            root: base.clone(),
            mods_dir: mods_dir.clone(),
            version: None,
            source: Source::Manual,
        };
        let mods = modlist::scan(&install).unwrap();
        let ids: BTreeSet<String> = ["BaseLib", "Char"].iter().map(|s| s.to_string()).collect();
        let params = PublishParams {
            set_name: "Test".into(),
            set_version: "0.0.1".into(),
            base_url: "https://x/".into(),
            published_at: String::new(),
            baselib_version: None,
        };

        let prep = prepare(&mods, &ids, &params).unwrap();
        // El manifest describe EXACTAMENTE lo instalado -> plan es noop (nada que bajar/borrar).
        let plan = sync::plan(&prep.manifest, &mods_dir).unwrap();
        assert!(
            plan.is_noop(),
            "esperaba noop: to_download={} orphans={}",
            plan.to_download.len(),
            plan.orphans.len()
        );
        // 5 archivos (BaseLib.json+dll, Char.json+dll, data.pck), todos contenido distinto.
        assert_eq!(prep.assets.len(), 5);
        let _ = std::fs::remove_dir_all(&base);
    }
}
