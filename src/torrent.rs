//! Backend P2P estilo torrent (librqbit) con fallback HTTP. Gateado tras la feature `p2p`.
//!
//! Los assets de un set ya son **content-addressed** (el archivo se llama por su BLAKE3),
//! asi que el torrent indexa exactamente esos nombres. La descarga P2P es una capa de
//! transporte mas: `sync::apply` igual verifica el BLAKE3 por archivo, asi que bajar de un
//! peer no confiable es seguro. Y como el magnet viaja DENTRO del set-manifest FIRMADO, un
//! atacante no puede sustituir el torrent.
//!
//! - **Publicar:** `create_set_torrent` arma el `.torrent` del dir de assets y devuelve el
//!   magnet (que `publish` mete en el manifest, antes de firmar).
//! - **Seedear:** `seed_blocking` levanta una session librqbit apuntando al dir de assets
//!   (archivos ya presentes) -> seedea sin re-bajar; corre hasta que `stop()` da true.
//! - **Sincronizar:** `HybridSource` implementa `transport::ModSource`: `prepare` se une al
//!   swarm y baja los archivos pedidos a un staging; `fetch` los mueve a destino. Si no hay
//!   seeder (swarm muerto), cae al `GitHubReleases` (HTTP).
//!
//! **P2P es OPT-IN** (`STS2_P2P=1` o peers manuales en `STS2_P2P_PEERS`). Por default la sync baja
//! por HTTP: el P2P solo ayuda si el publicador esta seedeando ACTIVAMENTE (raro), y un magnet sin
//! seeder hacia que `add_torrent` colgara resolviendo metadata -> la barra quedaba en 0% para siempre.

#![cfg(feature = "p2p")]

use crate::manifest::{FileEntry, SetManifest};
use crate::transport::{GitHubReleases, ModSource};
use anyhow::{Context, Result};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use librqbit::{
    AddTorrent, AddTorrentOptions, CreateTorrentOptions, ManagedTorrent, Session, SessionOptions,
    create_torrent,
};

/// Nombre interno del torrent (= subcarpeta donde caen los archivos al bajar/seedear). Fijo,
/// asi el layout en disco es `<output_folder>/assets/<blake3>` tanto al crear como al bajar.
const TORRENT_NAME: &str = "assets";

/// Trackers publicos abiertos (ademas del DHT que librqbit trae prendido) para discovery.
const TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.tracker.cl:1337/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
    "udp://exodus.desync.com:6969/announce",
];

/// Cuanto esperar a que aparezcan bytes (peer + metadata + datos) antes de declarar el swarm
/// muerto y caer a HTTP.
const SWARM_WAIT: Duration = Duration::from_secs(25);

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("creando runtime tokio para P2P")
}

/// Peers iniciales manuales (avanzado / LAN / tests): `STS2_P2P_PEERS=ip:port,ip:port`.
fn peers_env() -> Option<Vec<std::net::SocketAddr>> {
    let v = std::env::var("STS2_P2P_PEERS").ok()?;
    let peers: Vec<std::net::SocketAddr> =
        v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    (!peers.is_empty()).then_some(peers)
}

/// Desactivar DHT (tests/redes cerradas): `STS2_P2P_NODHT=1`.
fn nodht_env() -> bool {
    std::env::var("STS2_P2P_NODHT").is_ok()
}

/// El cliente INTENTA P2P solo si se opta explicitamente (`STS2_P2P=1`) o hay peers manuales
/// (`STS2_P2P_PEERS`). Por DEFAULT la sync baja por HTTP (GitHub Releases): el P2P solo ayuda si el
/// publicador esta seedeando, y un magnet sin seeder colgaba la descarga en 0%. El seedeo
/// (`seed_blocking`, lado modder) NO depende de esto.
fn p2p_opt_in() -> bool {
    std::env::var("STS2_P2P").is_ok() || peers_env().is_some()
}

/// Puerto fijo de escucha del seeder (tests/port-forward): `STS2_P2P_SEED_PORT=6881`.
fn seed_port_env() -> Option<std::ops::Range<u16>> {
    let p: u16 = std::env::var("STS2_P2P_SEED_PORT").ok()?.parse().ok()?;
    Some(p..p + 1)
}

/// percent-encode minimal para armar el magnet (todo lo no `A-Za-z0-9-_.~`).
fn pct(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                o.push(b as char)
            }
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

fn make_magnet(infohash_hex: &str, display_name: &str) -> String {
    let mut m = format!(
        "magnet:?xt=urn:btih:{infohash_hex}&dn={}",
        pct(display_name)
    );
    for tr in TRACKERS {
        m.push_str("&tr=");
        m.push_str(&pct(tr));
    }
    m
}

/// Crea el `.torrent` del dir de assets (archivos content-addressed por blake3). Devuelve
/// `(magnet, bytes_del_torrent)`. Lado modder (`publish`).
pub fn create_set_torrent(assets_dir: &Path, display_name: &str) -> Result<(String, Vec<u8>)> {
    let rt = runtime()?;
    rt.block_on(async {
        let res = create_torrent(
            assets_dir,
            CreateTorrentOptions {
                name: Some(TORRENT_NAME),
                piece_length: None,
            },
        )
        .await
        .context("creando el torrent del set")?;
        let bytes = res.as_bytes().context("serializando el .torrent")?.to_vec();
        let magnet = make_magnet(&res.info_hash().as_string(), display_name);
        Ok((magnet, bytes))
    })
}

/// Estado de seeding para la UI.
#[derive(Debug, Clone, Default)]
pub struct SeedStatus {
    pub state: String,
    pub uploaded_bytes: u64,
    pub complete: bool,
}

/// Seedea `assets_dir` para el torrent dado (bytes del `.torrent`). Bloquea: poll cada ~1s
/// llamando `on_status`, hasta que `stop()` devuelva true. Lado modder.
pub fn seed_blocking(
    assets_dir: &Path,
    torrent_bytes: &[u8],
    on_status: &mut dyn FnMut(SeedStatus),
    stop: &dyn Fn() -> bool,
) -> Result<()> {
    let rt = runtime()?;
    let bytes = torrent_bytes.to_vec(); // Vec<u8>: Into<Bytes>
    rt.block_on(async {
        let session = Session::new_with_opts(
            assets_dir.to_path_buf(),
            SessionOptions {
                disable_dht: nodht_env(),
                disable_dht_persistence: true,
                persistence: None,
                listen_port_range: seed_port_env(),
                ..Default::default()
            },
        )
        .await
        .context("creando session librqbit (seed)")?;
        let resp = session
            .add_torrent(
                AddTorrent::from_bytes(bytes),
                Some(AddTorrentOptions {
                    overwrite: true,
                    ..Default::default()
                }),
            )
            .await
            .context("agregando torrent para seedear")?;
        let handle = resp
            .into_handle()
            .context("el torrent no devolvio handle")?;
        loop {
            if stop() {
                break;
            }
            let st = handle.stats();
            on_status(SeedStatus {
                state: format!("{:?}", st.state),
                uploaded_bytes: st.uploaded_bytes,
                complete: st.finished,
            });
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        Ok::<(), anyhow::Error>(())
    })
}

/// Sesion de descarga P2P: runtime + staging temporal + handle del torrent agregado. Baja a
/// `staging/assets/<blake3>`; `HybridSource::fetch` copia cada archivo a destino.
struct TorrentSession {
    rt: tokio::runtime::Runtime,
    staging: PathBuf,
    magnet: String,
    handle: RefCell<Option<Arc<ManagedTorrent>>>,
    dead: Cell<bool>,
    prepared: Cell<bool>,
}

impl TorrentSession {
    fn new(magnet: &str) -> Result<Self> {
        let tag = blake3::hash(magnet.as_bytes()).to_hex();
        let staging = std::env::temp_dir().join(format!("sts2_modsync_p2p_{}", &tag[..16]));
        std::fs::create_dir_all(&staging)
            .with_context(|| format!("creando staging {}", staging.display()))?;
        Ok(Self {
            rt: runtime()?,
            staging,
            magnet: magnet.to_string(),
            handle: RefCell::new(None),
            dead: Cell::new(false),
            prepared: Cell::new(false),
        })
    }

    /// Busca el archivo `<blake3>` ya bajado dentro del staging (cae en `assets/<blake3>`).
    fn staged_file(&self, blake3: &str) -> Option<PathBuf> {
        let direct = self.staging.join(TORRENT_NAME).join(blake3);
        if direct.is_file() {
            return Some(direct);
        }
        // por las dudas (otra subcarpeta), buscar por nombre.
        walkdir::WalkDir::new(&self.staging)
            .into_iter()
            .filter_map(Result::ok)
            .find(|e| e.file_type().is_file() && e.file_name() == std::ffi::OsStr::new(blake3))
            .map(|e| e.path().to_path_buf())
    }

    /// Se une al swarm y baja SOLO los `entries` pedidos. Best-effort: si no hay seeder
    /// (sin bytes tras `SWARM_WAIT`) o algo falla, marca `dead` y vuelve Ok (fetch -> HTTP).
    fn download(&self, entries: &[FileEntry], on_bytes: &mut dyn FnMut(u64) -> bool) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        // regex que matchea exactamente los blake3 pedidos (hex => seguro en regex).
        let names = entries
            .iter()
            .map(|e| e.blake3.as_str())
            .collect::<Vec<_>>()
            .join("|");
        // Sin anclas: los blake3 son hex unicos de 64 chars (sin falsos positivos) y asi es
        // robusto si librqbit antepone el nombre del torrent al path ("assets/<blake3>").
        let regex = format!("({names})");
        let staging = self.staging.clone();
        let magnet = self.magnet.clone();
        let handle_cell = &self.handle;

        let res: Result<()> = self.rt.block_on(async {
            let session = Session::new_with_opts(
                staging,
                SessionOptions {
                    disable_dht: nodht_env(),
                    disable_dht_persistence: true,
                    persistence: None,
                    ..Default::default()
                },
            )
            .await
            .context("creando session librqbit (download)")?;
            // OJO: para un magnet, `add_torrent` resuelve la metadata del swarm/DHT ANTES de devolver
            // el handle. Sin seeder eso COLGABA para siempre (la barra en 0%), porque el SWARM_WAIT de
            // abajo solo aplica al poll posterior. Lo acotamos: si no hay metadata en SWARM_WAIT, se
            // aborta y cae a HTTP.
            let add = session.add_torrent(
                AddTorrent::from_url(magnet.as_str()),
                Some(AddTorrentOptions {
                    only_files_regex: Some(regex),
                    overwrite: true,
                    initial_peers: peers_env(),
                    ..Default::default()
                }),
            );
            let resp = match tokio::time::timeout(SWARM_WAIT, add).await {
                Ok(r) => r.context("agregando torrent (magnet)")?,
                Err(_) => {
                    anyhow::bail!(
                        "sin metadata del torrent (no hay seeder) en {SWARM_WAIT:?}; HTTP"
                    )
                }
            };
            let handle = resp
                .into_handle()
                .context("el torrent no devolvio handle")?;
            *handle_cell.borrow_mut() = Some(handle.clone());

            // Poll de progreso: reporto deltas a la barra; corto al terminar; declaro muerto
            // si no llega ni un byte en SWARM_WAIT (no hay seeder) o si stalea mucho.
            let mut last: u64 = 0;
            let mut last_change = Instant::now();
            loop {
                // Chequear cancelacion CADA vuelta (no solo cuando avanzan bytes): asi el boton
                // Cancelar responde aunque el swarm este stalleado. delta 0 no infla la barra.
                if !on_bytes(0) {
                    anyhow::bail!("descarga P2P cancelada");
                }
                let st = handle.stats();
                if let Some(err) = st.error {
                    anyhow::bail!("torrent en error: {err}");
                }
                if st.progress_bytes > last {
                    if !on_bytes(st.progress_bytes - last) {
                        anyhow::bail!("descarga P2P cancelada");
                    }
                    last = st.progress_bytes;
                    last_change = Instant::now();
                }
                if st.finished {
                    break;
                }
                if last_change.elapsed() > SWARM_WAIT {
                    anyhow::bail!("sin progreso P2P (no hay seeder); se usa HTTP");
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            Ok(())
        });

        if let Err(e) = res {
            eprintln!("[p2p] {e:#} -> fallback HTTP");
            self.dead.set(true);
        }
        Ok(())
    }
}

/// Fuente hibrida: intenta P2P (torrent) y cae a HTTP (GitHub Releases) si no hay seeder.
pub struct HybridSource {
    torrent: Option<TorrentSession>,
    http: GitHubReleases,
}

impl HybridSource {
    /// Construye desde el manifest: si se opto por P2P (`p2p_opt_in`) y el manifest trae `magnet`,
    /// arma la sesion torrent; siempre tiene el backend HTTP como fallback (y como DEFAULT).
    pub fn new(manifest: &SetManifest) -> Self {
        let torrent = if p2p_opt_in() {
            manifest
                .magnet
                .as_deref()
                .and_then(|m| match TorrentSession::new(m) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        eprintln!("[p2p] no se pudo iniciar la sesion torrent: {e:#} -> solo HTTP");
                        None
                    }
                })
        } else {
            None // default: HTTP directo (sin esperar a un swarm que casi nunca tiene seeder)
        };
        Self {
            torrent,
            http: GitHubReleases::new(),
        }
    }

    /// `true` si hay un magnet y la sesion P2P arranco (para que la UI muestre "via P2P").
    pub fn has_p2p(&self) -> bool {
        self.torrent.is_some()
    }
}

impl ModSource for HybridSource {
    fn prepare(&self, entries: &[FileEntry], on_bytes: &mut dyn FnMut(u64) -> bool) -> Result<()> {
        if let Some(ts) = &self.torrent
            && !ts.dead.get()
        {
            ts.download(entries, on_bytes)?;
            ts.prepared.set(true);
        }
        Ok(())
    }

    fn fetch(
        &self,
        base_url: &str,
        entry: &FileEntry,
        dest: &Path,
        on_bytes: &mut dyn FnMut(u64) -> bool,
    ) -> Result<()> {
        // Si el torrent bajo este archivo, copiarlo (bytes ya contados en `prepare` => 0 aca).
        if let Some(ts) = &self.torrent
            && ts.prepared.get()
            && !ts.dead.get()
            && let Some(src) = ts.staged_file(&entry.blake3)
        {
            std::fs::copy(&src, dest)
                .with_context(|| format!("copiando del staging P2P {}", src.display()))?;
            return Ok(());
        }
        // Fallback HTTP (o el archivo no vino por P2P).
        self.http.fetch(base_url, entry, dest, on_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnet_bien_formado() {
        let m = make_magnet("0123456789abcdef0123456789abcdef01234567", "Mi Set");
        assert!(m.starts_with("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"));
        assert!(m.contains("&dn=Mi%20Set"));
        assert!(m.contains("&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce"));
    }

    #[test]
    fn create_torrent_round_trip() {
        // Arma un dir de assets de prueba y crea el torrent; el magnet debe traer el infohash.
        let dir = std::env::temp_dir().join("sts2_modsync_torrent_test");
        let _ = std::fs::remove_dir_all(&dir);
        let assets = dir.join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        // dos "assets" content-addressed (nombre cualquiera; el torrent no exige que sea blake3).
        std::fs::write(assets.join("aaaa"), b"contenido uno").unwrap();
        std::fs::write(assets.join("bbbb"), b"contenido dos distinto").unwrap();

        let (magnet, bytes) = create_set_torrent(&assets, "Set De Prueba").unwrap();
        assert!(magnet.starts_with("magnet:?xt=urn:btih:"));
        assert!(magnet.contains("&dn=Set%20De%20Prueba"));
        assert!(!bytes.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// E2E real de P2P en loopback: crea un set, lo seedea en un thread (puerto fijo, sin DHT)
    /// y lo baja con `HybridSource` apuntando al seeder local. El `base_url` es invalido a
    /// proposito: si bajara por HTTP fallaria -> si el test pasa, BAJO POR P2P de verdad.
    /// Ignorado por default (abre sockets); correr con:
    ///   cargo test --features p2p p2p_loopback -- --ignored --nocapture --test-threads=1
    #[test]
    #[ignore = "loopback P2P real (abre sockets); correr con --ignored --test-threads=1"]
    fn p2p_loopback_seed_y_download() {
        use crate::manifest::{FileEntry, ModEntry, SetManifest};
        use std::sync::atomic::{AtomicBool, Ordering};
        if crate::detect::is_game_running() {
            eprintln!("(skip: Slay the Spire 2 esta abierto)");
            return;
        }

        let base = std::env::temp_dir().join("sts2_modsync_p2p_loopback");
        let _ = std::fs::remove_dir_all(&base);
        let assets = base.join("pub").join("assets");
        std::fs::create_dir_all(&assets).unwrap();

        // Assets content-addressed: nombre = su blake3 real (como en produccion).
        let contents: [&[u8]; 2] = [
            b"contenido P2P uno",
            b"otro contenido distinto para el segundo archivo",
        ];
        let mut hashes = Vec::new();
        for c in contents {
            let h = blake3::hash(c).to_hex().to_string();
            std::fs::write(assets.join(&h), c).unwrap();
            hashes.push((h, c.len() as u64));
        }

        let (magnet, torrent_bytes) = create_set_torrent(&assets, "Loopback Set").unwrap();

        let files: Vec<FileEntry> = hashes
            .iter()
            .enumerate()
            .map(|(i, (h, sz))| FileEntry {
                path: format!("Mod/file{i}.bin"),
                size: *sz,
                blake3: h.clone(),
                deltas: Vec::new(),
            })
            .collect();
        let manifest = SetManifest {
            schema: 1,
            set_name: "Loopback".into(),
            set_version: "1".into(),
            published_at: "now".into(),
            signing_key_id: None,
            base_url: "https://invalid.invalid/".into(), // si cae a HTTP, falla a proposito
            magnet: Some(magnet),
            baselib_version: None,
            mods: vec![ModEntry {
                id: "Mod".into(),
                version: "1".into(),
                dependencies: vec![],
                files,
            }],
        };

        // Envs ANTES de spawnear (evita set_var concurrente): seeder en :6899, cliente apunta
        // a ese peer, ambos sin DHT.
        unsafe {
            std::env::set_var("STS2_P2P_SEED_PORT", "6899");
            std::env::set_var("STS2_P2P_PEERS", "127.0.0.1:6899");
            std::env::set_var("STS2_P2P_NODHT", "1");
        }

        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let assets2 = assets.clone();
        let seeder = std::thread::spawn(move || {
            let _ = seed_blocking(&assets2, &torrent_bytes, &mut |_| {}, &|| {
                stop2.load(Ordering::Relaxed)
            });
        });
        std::thread::sleep(Duration::from_secs(2)); // que el seeder levante el listener

        let mods_dir = base.join("mods");
        std::fs::create_dir_all(&mods_dir).unwrap();
        let plan = crate::sync::plan(&manifest, &mods_dir).unwrap();
        assert_eq!(plan.to_download.len(), 2, "deberian faltar los 2 archivos");

        let source = HybridSource::new(&manifest);
        assert!(
            source.has_p2p(),
            "el manifest tiene magnet -> debe haber P2P"
        );
        crate::sync::apply(
            &plan,
            &manifest,
            &mods_dir,
            &source,
            &mut |_| {},
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .expect("apply via P2P deberia funcionar");

        stop.store(true, Ordering::Relaxed);
        let _ = seeder.join();

        for (i, c) in contents.iter().enumerate() {
            let p = mods_dir.join("Mod").join(format!("file{i}.bin"));
            assert!(p.is_file(), "falta {} (no bajo por P2P)", p.display());
            assert_eq!(&std::fs::read(&p).unwrap(), c, "contenido distinto en {i}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
