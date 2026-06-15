//! Modo PUBLICAR (lado modder): genera un set-manifest desde los mods INSTALADOS, hasheando
//! cada archivo, y junta los assets (content-addressed por BLAKE3) listos para subir a un
//! GitHub Release. Es el inverso de `sync::plan`: el modder corre esto, sube el resultado, y
//! sus amigos lo sincronizan. Reusa `modlist` (mods instalados) + `hashing` (BLAKE3).

use crate::hashing;
use crate::manifest::{
    BASELIB_ID, Delta, FileEntry, LOAD_ORDER_ENFORCER_ID, ModEntry, SetManifest,
};
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
            files.push(FileEntry {
                path,
                size,
                blake3,
                deltas: Vec::new(),
            });
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
        magnet: None,
        baselib_version: p.baselib_version.clone(),
        mods: entries,
    };
    manifest
        .validate()
        .context("el manifest generado no valida")?;
    Ok(Prepared { manifest, assets })
}

/// Tope de tamaño para GENERAR un delta. Arriba de esto, bsdiff (suffix array sobre el archivo
/// viejo) usa demasiada RAM/CPU para que valga la pena en un publish. Los `.pck` tipicos van debajo.
const DELTA_MAX_FILE: u64 = 600 * 1024 * 1024;

/// Resumen de la generacion de deltas (para informarle el ahorro al modder).
#[derive(Debug, Default)]
pub struct DeltaReport {
    /// Cantidad de patches generados.
    pub patches: usize,
    /// Bytes totales de los patches (lo que un amigo baja en vez del full).
    pub patch_bytes: u64,
    /// Bytes totales de los archivos full que esos patches reemplazan (el "antes").
    pub full_bytes: u64,
}

/// Genera patches bsdiff de cada archivo CAMBIADO contra la PUBLICACION ANTERIOR que haya en
/// `out_dir` (su `set-manifest.json` + `assets/<old_blake3>`), y los agrega a `prep`: el `Delta` al
/// `FileEntry` del manifest y el patch como asset en `assets/<patch_blake3>` (que se sube con todo).
/// Asi, al actualizar, un amigo que YA tiene la version vieja baja solo el diff. No-op (sin error)
/// si no hay publicacion previa en `out_dir` o si nada cambio. Un patch se DESCARTA si no resulta
/// mas chico que el archivo full (ahi el cliente baja el full igual). Llamar ANTES de `write_out`
/// (que sobreescribe el manifest viejo). Es 100% opcional: sin deltas, la sync baja el full.
pub fn add_deltas(prep: &mut Prepared, out_dir: &Path) -> Result<DeltaReport> {
    let mut report = DeltaReport::default();
    // Manifest de la publicacion anterior en este out_dir (si lo hay): path -> old_blake3.
    let Ok(prev_text) = std::fs::read_to_string(out_dir.join("set-manifest.json")) else {
        return Ok(report); // primera publicacion en este out_dir: nada contra que diffear
    };
    let Ok(prev) = SetManifest::from_json_str(&prev_text) else {
        return Ok(report); // manifest viejo ilegible: no romper el publish por esto
    };
    let prev_hash: BTreeMap<&str, &str> = prev
        .mods
        .iter()
        .flat_map(|m| m.files.iter().map(|f| (f.path.as_str(), f.blake3.as_str())))
        .collect();
    let assets_dir = out_dir.join("assets");
    std::fs::create_dir_all(&assets_dir).ok();
    // new_blake3 -> ruta de los bytes NUEVOS (los assets que `prepare` ya listo). Owned para no
    // mantener prestado `prep.assets` mientras mutamos `prep.manifest`.
    let new_src: BTreeMap<String, PathBuf> = prep
        .assets
        .iter()
        .map(|a| (a.blake3.clone(), a.src.clone()))
        .collect();
    let mut patch_assets: Vec<Asset> = Vec::new();
    // blake3 ya emitidos (assets full de `prepare` + patches ya generados): evita subir/contar dos
    // veces el MISMO patch si dos archivos producen el mismo diff (mismo old+new -> mismo patch).
    let mut emitted: BTreeSet<String> = prep.assets.iter().map(|a| a.blake3.clone()).collect();

    for m in &mut prep.manifest.mods {
        for f in &mut m.files {
            if f.size > DELTA_MAX_FILE {
                continue;
            }
            let Some(old_hash) = prev_hash.get(f.path.as_str()) else {
                continue;
            };
            if old_hash.eq_ignore_ascii_case(&f.blake3) {
                continue; // el archivo no cambio
            }
            let old_asset = assets_dir.join(old_hash);
            let Some(new_path) = new_src.get(&f.blake3) else {
                continue;
            };
            if !old_asset.is_file() {
                continue; // no tenemos los bytes viejos (assets viejos limpiados): sin delta
            }
            let (Ok(old_bytes), Ok(new_bytes)) =
                (std::fs::read(&old_asset), std::fs::read(new_path))
            else {
                continue;
            };
            let Ok(patch) = crate::delta::diff(&old_bytes, &new_bytes) else {
                continue;
            };
            // Solo vale la pena si el patch es MAS CHICO que el archivo completo.
            if patch.len() as u64 >= f.size {
                continue;
            }
            let patch_blake3 = crate::hashing::blake3_bytes(&patch);
            let patch_dst = assets_dir.join(&patch_blake3);
            if !patch_dst.exists() {
                std::fs::write(&patch_dst, &patch)
                    .with_context(|| format!("escribiendo patch {}", patch_dst.display()))?;
            }
            // El delta va SIEMPRE al FileEntry (este archivo lo puede usar)...
            f.deltas.push(Delta {
                from_blake3: (*old_hash).to_string(),
                patch_blake3: patch_blake3.clone(),
                patch_size: patch.len() as u64,
            });
            report.patches += 1;
            report.full_bytes += f.size;
            // ...pero el ASSET del patch (y sus bytes) UNA sola vez aunque dos archivos lo compartan.
            if emitted.insert(patch_blake3.clone()) {
                report.patch_bytes += patch.len() as u64;
                patch_assets.push(Asset {
                    blake3: patch_blake3,
                    src: patch_dst,
                    size: patch.len() as u64,
                });
            }
        }
    }
    prep.assets.extend(patch_assets);
    Ok(report)
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

    // Con feature p2p: armar el torrent del dir de assets y meter el magnet en el manifest
    // ANTES de serializar/firmar (asi la firma cubre el magnet). Tambien deja `set.torrent`
    // local para seedear. Sin la feature, el manifest sale sin magnet (solo HTTP).
    #[cfg_attr(not(feature = "p2p"), allow(unused_mut))]
    let mut manifest = prep.manifest.clone();
    #[cfg(feature = "p2p")]
    {
        let (magnet, torrent_bytes) =
            crate::torrent::create_set_torrent(&assets_dir, &manifest.set_name)
                .context("creando el torrent del set")?;
        std::fs::write(out_dir.join("set.torrent"), &torrent_bytes)
            .with_context(|| format!("escribiendo {}", out_dir.join("set.torrent").display()))?;
        manifest.magnet = Some(magnet);
    }

    let manifest_path = out_dir.join("set-manifest.json");
    let json = serde_json::to_string_pretty(&manifest)?;
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

fn run_gh(args: &[&str]) -> Result<std::process::Output> {
    std::process::Command::new("gh")
        .args(args)
        .output()
        .context("no se pudo ejecutar `gh` (¿esta instalado y logueado el GitHub CLI?)")
}

/// Sube el contenido de `out_dir` (set-manifest.json + .minisig + assets/*) al GitHub Release
/// derivado de `base_url`. Si hay un token de GitHub guardado (login en la app), sube por la
/// **API REST** (sin depender del `gh` CLI); si no, cae al `gh` CLI. Devuelve la URL del release.
pub fn upload(out_dir: &Path, base_url: &str) -> Result<String> {
    if let Some(token) = crate::github::load_token() {
        return upload_via_api(out_dir, base_url, &token);
    }
    upload_via_gh(out_dir, base_url)
}

/// Sube por la API REST de GitHub (token guardado en el llavero). Crea el repo del usuario si
/// falta, crea/usa el release del tag, y sube con clobber el manifest + firma + torrent + assets.
fn upload_via_api(out_dir: &Path, base_url: &str, token: &str) -> Result<String> {
    let (owner, repo, tag) = crate::github::parse_release_base_url(base_url).context(
        "el base_url no es una URL de release de GitHub \
         (https://github.com/<owner>/<repo>/releases/download/<tag>/)",
    )?;
    let api = crate::github::Api::new(token.to_string());
    let login = api.whoami().context("validando el token de GitHub")?;
    // Crear el repo SOLO si va bajo el usuario del token (POST /user/repos crea ahi). Si el owner
    // es una org u otro usuario, el repo debe existir; el release dara un error claro si no.
    if owner.eq_ignore_ascii_case(&login) {
        api.ensure_repo(&repo)
            .context("creando el repo en GitHub")?;
    }
    let files = crate::github::collect_upload_files(out_dir);
    api.publish_assets(&owner, &repo, &tag, &files, |_, _| {})
}

/// Sube via el `gh` CLI (fallback si no hay token guardado). Devuelve la URL del release.
fn upload_via_gh(out_dir: &Path, base_url: &str) -> Result<String> {
    let (owner, repo, tag) = crate::github::parse_release_base_url(base_url).context(
        "el base_url no es una URL de release de GitHub \
         (https://github.com/<owner>/<repo>/releases/download/<tag>/) — subi a mano",
    )?;
    let repo_arg = format!("{owner}/{repo}");

    // 1) Crear el release (si ya existe, gh falla -> se ignora; el release existe).
    let _ = run_gh(&[
        "release",
        "create",
        &tag,
        "--repo",
        &repo_arg,
        "--title",
        &tag,
        "--notes",
        "Set de mods publicado con sts2-modsync.",
    ]);

    // 2) Juntar archivos: manifest + firma (si esta) + todos los assets.
    let mut files: Vec<PathBuf> = vec![out_dir.join("set-manifest.json")];
    let sig = out_dir.join("set-manifest.json.minisig");
    if sig.exists() {
        files.push(sig);
    }
    let torrent = out_dir.join("set.torrent");
    if torrent.exists() {
        files.push(torrent);
    }
    if let Ok(rd) = std::fs::read_dir(out_dir.join("assets")) {
        for e in rd.flatten() {
            if e.path().is_file() {
                files.push(e.path());
            }
        }
    }

    // 3) Subir en lotes (limite de longitud de comando en Windows).
    for batch in files.chunks(40) {
        let mut args: Vec<String> = vec![
            "release".into(),
            "upload".into(),
            tag.clone(),
            "--repo".into(),
            repo_arg.clone(),
            "--clobber".into(),
        ];
        for f in batch {
            args.push(f.to_string_lossy().to_string());
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run_gh(&refs)?;
        if !out.status.success() {
            bail!(
                "gh release upload fallo: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }

    Ok(format!(
        "https://github.com/{owner}/{repo}/releases/tag/{tag}"
    ))
}

/// Comando sugerido para subir todo a un GitHub Release con el `gh` CLI (fallback si no se puede
/// subir automaticamente). El tag = `<tag>` del `base_url`. Incluye el `.minisig` si se firmo.
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

    #[test]
    fn add_deltas_genera_patch_contra_la_publicacion_anterior() {
        let base = std::env::temp_dir().join("sts2_modsync_publish_delta");
        let _ = std::fs::remove_dir_all(&base);
        let mods_dir = base.join("mods");
        let out_dir = base.join("out");
        let assets_dir = out_dir.join("assets");
        std::fs::create_dir_all(&assets_dir).unwrap();

        // .pck grande casi-identico entre v1 y v2 (asi el patch sale MUCHO mas chico que el full).
        let big_v1 = b"contenido grande del .pck de un mod ".repeat(2000);
        let mut big_v2 = big_v1.clone();
        big_v2[1000] = b'Z';
        big_v2.extend_from_slice(b" + un poquito de contenido nuevo apendido al final");
        let v1_hash = crate::hashing::blake3_bytes(&big_v1);

        // Simular la publicacion ANTERIOR en out_dir: su set-manifest.json + el asset viejo.
        let prev = SetManifest {
            schema: 1,
            set_name: "T".into(),
            set_version: "1".into(),
            published_at: "x".into(),
            signing_key_id: None,
            base_url: "https://x/".into(),
            magnet: None,
            baselib_version: None,
            mods: vec![ModEntry {
                id: "Char".into(),
                version: "1".into(),
                dependencies: vec![],
                files: vec![FileEntry {
                    path: "Char/data.pck".into(),
                    size: big_v1.len() as u64,
                    blake3: v1_hash.clone(),
                    deltas: vec![],
                }],
            }],
        };
        std::fs::write(
            out_dir.join("set-manifest.json"),
            serde_json::to_string(&prev).unwrap(),
        )
        .unwrap();
        std::fs::write(assets_dir.join(&v1_hash), &big_v1).unwrap();

        // Los mods en disco tienen la version NUEVA.
        make_mod(&mods_dir, "Char", &[("data.pck", &big_v2)]);
        let install = Install {
            root: base.clone(),
            mods_dir: mods_dir.clone(),
            version: None,
            source: Source::Manual,
        };
        let ids: BTreeSet<String> = ["Char"].iter().map(|s| s.to_string()).collect();
        let mods = modlist::scan(&install).unwrap();
        let params = PublishParams {
            set_name: "T".into(),
            set_version: "2".into(),
            base_url: "https://x/".into(),
            published_at: String::new(),
            baselib_version: None,
        };
        let mut prep = prepare(&mods, &ids, &params).unwrap();

        let r = add_deltas(&mut prep, &out_dir).unwrap();
        assert_eq!(
            r.patches, 1,
            "deberia generar 1 patch para el .pck cambiado"
        );
        assert!(
            r.patch_bytes > 0 && r.patch_bytes < r.full_bytes,
            "el patch deberia ser mas chico que el full (patch={} full={})",
            r.patch_bytes,
            r.full_bytes
        );

        // El FileEntry del .pck quedo con un delta desde el hash viejo, y el patch esta en assets/.
        let f = prep
            .manifest
            .mods
            .iter()
            .flat_map(|m| &m.files)
            .find(|f| f.path == "Char/data.pck")
            .unwrap();
        assert_eq!(f.deltas.len(), 1);
        assert_eq!(f.deltas[0].from_blake3, v1_hash);
        assert!(
            assets_dir.join(&f.deltas[0].patch_blake3).is_file(),
            "el patch deberia haberse escrito a assets/"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
