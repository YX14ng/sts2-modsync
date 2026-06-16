// Single-exe: con `--features gui`, ESTE binario es la app entera (GUI + CLI). El subsistema
// "windows" evita la consola negra al abrir el GUI con doble-clic; en modo CLI nos enganchamos
// a la consola del padre (`attach_parent_console`) para que se vea la salida. Sin la feature
// `gui` (build liviano de CLI), queda como consola normal.
#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]

//! sts2-modsync — mod manager (+ sync) de Slay the Spire 2. **Single-exe**: con `--features gui`,
//! sin argumentos abre el GUI (doble-clic); con subcomandos corre la CLI. Sin la feature, solo CLI.
//!
//! Subcomandos:
//!   sts2-modsync [list]            lista los mods instalados (habilitados/deshabilitados)
//!   sts2-modsync enable  <id>      habilita un mod (mueve la carpeta a mods/)
//!   sts2-modsync disable <id>      deshabilita un mod (mueve la carpeta a mods_disabled/)
//!   sts2-modsync launch            lanza el juego
//!   sts2-modsync sync    <set.json> dry-run del plan de sincronizacion de un set
//!
//! La GUI (mod manager con pestañas) se abre corriendo el exe sin argumentos, o en dev con
//! `cargo run --features gui`.

use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::path::Path;
use sts2_modsync::{
    config, detect, launch, manager, manifest::SetManifest, modlist, modsource::ModSource,
    modupdate, profile, publish, signing, sync, transport, update,
};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Sin argumentos en el build con GUI (doble-clic): abrir la interfaz grafica.
    #[cfg(feature = "gui")]
    if args.is_empty() {
        return sts2_modsync::gui::run().map_err(|e| anyhow::anyhow!("error en el GUI: {e}"));
    }
    // Modo CLI en el exe "windows" (sin consola propia): engancharse a la del padre para que
    // la salida (println/eprintln) se vea cuando se corre desde una terminal.
    #[cfg(all(windows, feature = "gui"))]
    attach_parent_console();

    let cmd = args.first().map(String::as_str).unwrap_or("list");

    if matches!(cmd, "help" | "-h" | "--help") {
        print_help();
        return Ok(());
    }
    // Self-test del auto-update (lo invoca `update::apply` sobre el exe nuevo): salir 0 rapido.
    if matches!(cmd, "--health-check" | "health-check") {
        println!("sts2-modsync {} OK", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if cmd == "update" {
        return cmd_update();
    }
    if cmd == "keygen" {
        return cmd_keygen();
    }
    if cmd == "sign" {
        return cmd_sign(&args);
    }
    if cmd == "seed" {
        return cmd_seed(&args);
    }
    if matches!(cmd, "github-login" | "github-logout" | "github-status") {
        return cmd_github(cmd, &args);
    }
    if matches!(cmd, "nexus-login" | "nexus-logout" | "nexus-status") {
        return cmd_nexus(cmd, &args);
    }
    if matches!(cmd, "nxm" | "nxm-register" | "nxm-unregister") {
        return cmd_nxm(cmd, &args);
    }

    let cfg = config::load();
    let Some(install) = resolve_install(&cfg) else {
        eprintln!("No se encontro Slay the Spire 2 y no se eligio carpeta. Abortando.");
        std::process::exit(1);
    };
    cache_install(&cfg, &install);

    match cmd {
        "list" => cmd_list(&install)?,
        "enable" => {
            let id = arg(&args, 1)?;
            manager::enable(&install, id)?;
            println!("habilitado: {id}");
        }
        "disable" => {
            let id = arg(&args, 1)?;
            manager::disable(&install, id)?;
            println!("deshabilitado: {id}");
        }
        "launch" => {
            // Por Steam (overlay) salvo `--direct` o que la config lo tenga apagado.
            let via_steam = cfg.launch_via_steam && !args.iter().any(|a| a == "--direct");
            launch::launch(&install, via_steam)?;
            println!("lanzando Slay the Spire 2...");
        }
        "publish" => cmd_publish(&install, &args)?,
        "dedupe" => cmd_dedupe(&install)?,
        "loadcode" => cmd_loadcode(&install, &args)?,
        "mod-source" => cmd_mod_source(&args)?,
        "mod-check" => cmd_mod_check(&install, &args)?,
        "mod-update" => cmd_mod_update(&install, &args)?,
        "sync" => cmd_sync(&install, arg(&args, 1)?)?,
        // compat: `sts2-modsync algo.json|http...` == `sync ...` (como el MVP viejo).
        other
            if (other.ends_with(".json") && Path::new(other).exists())
                || other.starts_with("http") =>
        {
            cmd_sync(&install, other)?
        }
        other => {
            bail!(
                "subcomando desconocido: {other:?} (probá: list|enable|disable|launch|sync|mod-check|mod-update|dedupe|loadcode|seed|help)"
            )
        }
    }
    Ok(())
}

/// En el exe con subsistema "windows" (sin consola propia) nos enganchamos a la consola del
/// proceso PADRE (la terminal desde donde se invoco) para que la salida del modo CLI sea
/// visible. Best-effort: si no hay consola padre (doble-clic), AttachConsole falla y no pasa nada.
#[cfg(all(windows, feature = "gui"))]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(dw_process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX; // (DWORD)-1
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// config cacheada (re-validada) -> deteccion automatica -> dialogo manual.
fn resolve_install(cfg: &config::Config) -> Option<detect::Install> {
    if let Some(root) = &cfg.install_root
        && let Some(i) = detect::from_root(root)
    {
        return Some(i);
    }
    detect::detect().or_else(detect::pick_folder_dialog)
}

fn cache_install(cfg: &config::Config, install: &detect::Install) {
    if cfg.install_root.as_deref() != Some(install.root.as_path()) {
        let mut cfg = cfg.clone();
        cfg.install_root = Some(install.root.clone());
        let _ = config::save(&cfg);
    }
}

fn arg(args: &[String], i: usize) -> Result<&str> {
    args.get(i)
        .map(String::as_str)
        .context("falta un argumento")
}

fn cmd_list(install: &detect::Install) -> Result<()> {
    print_install(install);
    let mods = modlist::scan(install)?;
    let (enabled, disabled): (Vec<_>, Vec<_>) = mods.iter().partition(|m| m.enabled);

    println!("\nMods habilitados ({}):", enabled.len());
    for m in &enabled {
        print_mod(m);
    }
    println!("\nMods deshabilitados ({}):", disabled.len());
    for m in &disabled {
        print_mod(m);
    }

    let missing = modlist::missing_dependencies(&mods);
    if !missing.is_empty() {
        println!("\n[!] Dependencias faltantes:");
        for (m, d) in &missing {
            println!("    {m} necesita {d} (no instalado o deshabilitado)");
        }
    }
    let conflicts = modlist::conflicts(&mods);
    if !conflicts.is_empty() {
        println!(
            "\n[!] Conflictos (ids duplicados): {}",
            conflicts.join(", ")
        );
    }

    println!(
        "\nOrden de carga (multiplayer): {}",
        modlist::load_order(&mods).join(" -> ")
    );
    if !modlist::load_order_enforced(&mods) {
        println!(
            "[!] ModListSorter no esta habilitado: el orden de carga puede divergir entre\n    amigos y romper el lobby (room-hash de BaseLib)."
        );
    }
    Ok(())
}

/// `dedupe`: limpia mods DUPLICADOS (mismo id en >1 carpeta) — conserva la version mas nueva y manda
/// las otras a la papelera (reversible). Imprime lo que hace.
fn cmd_dedupe(install: &detect::Install) -> Result<()> {
    let mods = modlist::scan(install)?;
    let groups = modlist::duplicates(&mods);
    if groups.is_empty() {
        println!("no hay mods duplicados.");
        return Ok(());
    }
    let mut removed = 0usize;
    for g in &groups {
        println!(
            "{}: conservo v{} ({})",
            g.id,
            g.keep.manifest.version.as_deref().unwrap_or("?"),
            g.keep.dir.display(),
        );
        for m in &g.remove {
            match manager::trash_mod_dir(install, &m.dir) {
                Ok(()) => {
                    println!(
                        "  - papelera: {} (v{})",
                        m.dir.display(),
                        m.manifest.version.as_deref().unwrap_or("?")
                    );
                    removed += 1;
                }
                Err(e) => eprintln!("  [!] {}: {e:#}", m.dir.display()),
            }
        }
    }
    println!("listo: {removed} duplicado(s) a la papelera.");
    Ok(())
}

/// `loadcode`: SIN argumento imprime el codigo compartible de la lista activa (que mods estan
/// habilitados); CON un `<codigo>` lo APLICA (habilita esos, deshabilita el resto). No baja archivos:
/// el amigo ya tiene que tener los mods. El orden de carga canonico sale solo.
fn cmd_loadcode(install: &detect::Install, args: &[String]) -> Result<()> {
    use sts2_modsync::loadcode;
    match args.get(1) {
        None => {
            let mods = modlist::scan(install)?;
            let ids: Vec<String> = mods
                .iter()
                .filter(|m| m.enabled)
                .map(|m| m.id().to_string())
                .collect();
            let code = loadcode::encode("", &ids);
            println!(
                "Codigo de la lista actual ({} mods activos) — pasaselo a un amigo (lo aplica con \
                 `loadcode <codigo>` o pegandolo en la pestaña Perfiles):\n\n{code}\n",
                ids.len()
            );
        }
        Some(code) => {
            let (name, ids) = loadcode::decode(code)?;
            let prof = profile::Profile {
                name: if name.trim().is_empty() {
                    "codigo".into()
                } else {
                    name.clone()
                },
                enabled_ids: ids,
            };
            let r = profile::apply(install, &prof)?;
            println!(
                "Lista{} aplicada: +{} activados, -{} desactivados.",
                if name.trim().is_empty() {
                    String::new()
                } else {
                    format!(" \"{name}\"")
                },
                r.enabled.len(),
                r.disabled.len()
            );
            if !r.not_installed.is_empty() {
                println!(
                    "  Faltan {} (no instalados, no se pudieron activar): {}",
                    r.not_installed.len(),
                    r.not_installed.join(", ")
                );
            }
        }
    }
    Ok(())
}

/// `keygen`: genera el par minisign del modder, guarda la secreta fuera del repo e imprime
/// la clave publica para pegar en `signing::PUBLISHER_PUBKEY`.
fn cmd_keygen() -> Result<()> {
    let path = signing::secret_key_path().context("no se pudo resolver el path de la clave")?;
    if path.exists() {
        bail!(
            "ya existe una clave en {} (borrala si querés regenerar)",
            path.display()
        );
    }
    let (sk, pk) = signing::generate_keypair()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &sk).with_context(|| format!("escribiendo {}", path.display()))?;
    println!("Clave SECRETA guardada en: {}", path.display());
    println!("(NO la compartas ni la subas al repo — con ella `publish` firma tus sets.)\n");
    println!("Pega esta clave PUBLICA en src/signing.rs (PUBLISHER_PUBKEY) y recompila:\n");
    println!("  {pk}\n");
    Ok(())
}

/// `sign <archivo>`: firma un archivo (escribe `<archivo>.minisig`). La clave secreta sale
/// de la env `MINISIGN_SECRET_KEY` (la usa el CI) o de la guardada por `keygen`. Sin clave,
/// no hace nada (sale OK) — asi el paso de firma del CI es opcional.
fn cmd_sign(args: &[String]) -> Result<()> {
    let file = arg(args, 1)?;
    let sk = std::env::var("MINISIGN_SECRET_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(signing::load_secret_key);
    let Some(sk) = sk else {
        println!("sin clave secreta (env MINISIGN_SECRET_KEY ni keygen) — no se firma.");
        return Ok(());
    };
    let data = std::fs::read(file).with_context(|| format!("leyendo {file}"))?;
    let sig = signing::sign(&sk, &data)?;
    let sig_path = format!("{file}.minisig");
    std::fs::write(&sig_path, sig).with_context(|| format!("escribiendo {sig_path}"))?;
    println!("firmado: {sig_path}");
    Ok(())
}

/// `seed <out_dir>`: seedea por P2P (torrent) el set publicado en `out_dir` (necesita
/// `set.torrent` + `assets/`, generados por `publish`). Bloquea hasta Ctrl-C: tus amigos
/// bajan de vos mientras corra. Requiere compilar con `--features p2p`.
#[cfg(feature = "p2p")]
fn cmd_seed(args: &[String]) -> Result<()> {
    let out_dir = Path::new(arg(args, 1)?);
    let torrent_path = out_dir.join("set.torrent");
    let assets_dir = out_dir.join("assets");
    let torrent_bytes = std::fs::read(&torrent_path).with_context(|| {
        format!(
            "no se pudo leer {} (¿corriste `publish` con --features p2p?)",
            torrent_path.display()
        )
    })?;
    if !assets_dir.is_dir() {
        bail!("no existe {} (assets del set)", assets_dir.display());
    }
    println!(
        "Seedeando {} ... (Ctrl-C para cortar)",
        assets_dir.display()
    );
    sts2_modsync::torrent::seed_blocking(
        &assets_dir,
        &torrent_bytes,
        &mut |st| {
            println!(
                "  estado: {:<14} subido: {:.1} MB{}",
                st.state,
                st.uploaded_bytes as f64 / 1_048_576.0,
                if st.complete { " (completo)" } else { "" }
            );
        },
        &|| false, // corre hasta Ctrl-C
    )
}

#[cfg(not(feature = "p2p"))]
fn cmd_seed(_args: &[String]) -> Result<()> {
    bail!("`seed` necesita P2P: recompila con `cargo build --features p2p` (o usa el GUI).")
}

fn cmd_sync(install: &detect::Install, src: &str) -> Result<()> {
    use sts2_modsync::github;
    print_install(install);
    // `sync repo:owner/repo` (o un `owner/repo` suelto que no sea un archivo): sigue el ULTIMO
    // release del repo (resuelve la URL del manifest), igual que la suscripcion por repo del GUI.
    // Para el `owner/repo` suelto exigimos que NO parezca un path a un archivo (un solo `/`, sin
    // `.json` ni `\`), asi un typo de ruta da "no such file" y no un 404 confuso contra la API.
    let looks_like_file =
        src.ends_with(".json") || src.contains('\\') || src.matches('/').count() != 1;
    let repo = config::as_repo_sub(src)
        .and_then(github::normalize_repo)
        .or_else(|| {
            (!src.starts_with("http") && !looks_like_file && !Path::new(src).exists())
                .then(|| github::normalize_repo(src))
                .flatten()
        });
    let resolved = match &repo {
        Some(owner_repo) => {
            let (owner, name) = owner_repo.split_once('/').context("repo invalido")?;
            let url = transport::resolve_latest_manifest(owner, name)?;
            println!("Repo {owner_repo}: ultimo release -> {url}\n");
            Some(url)
        }
        None => None,
    };
    let src = resolved.as_deref().unwrap_or(src);
    let (text, sig) = if src.starts_with("http") {
        let t = transport::get_text(src)?;
        let s = transport::get_text(&format!("{src}.minisig")).ok(); // firma opcional
        (t, s)
    } else {
        let t = std::fs::read_to_string(src)?;
        let s = std::fs::read_to_string(format!("{src}.minisig")).ok();
        (t, s)
    };
    let sig_status = signing::verify_optional(text.as_bytes(), sig.as_deref())?;
    let manifest = SetManifest::from_json_str(&text)?;
    println!(
        "\nSet: {} v{}  ({} mods)",
        manifest.set_name,
        manifest.set_version,
        manifest.mods.len()
    );
    match sig_status {
        signing::SigStatus::Verified => {
            println!("  firma: VERIFICADA OK (publicador de confianza)")
        }
        signing::SigStatus::Unsigned => {
            println!("  firma: SIN FIRMA — confias en la URL/HTTPS del publicador")
        }
        signing::SigStatus::DevUnverified => println!("  firma: NO verificada (modo dev)"),
    }
    if let Some(bl) = &manifest.baselib_version {
        println!("  BaseLib esperada: {bl}");
    }
    let plan = sync::plan(&manifest, &install.mods_dir)?;
    print_plan(&plan);
    Ok(())
}

/// `publish --name <set> --version <ver> (--repo <owner/repo> | --base-url <url>) [--profile <p>] [--out <dir>]`
/// El `--repo` se RECUERDA: la proxima vez podes omitirlo y publica otro release en el mismo repo.
fn cmd_publish(install: &detect::Install, args: &[String]) -> Result<()> {
    use sts2_modsync::github;
    let name = flag(args, "--name").context("falta --name")?;
    let version = flag(args, "--version").context("falta --version")?;
    let out = flag(args, "--out").unwrap_or("./set-publish");

    // Resolver base_url + repo: --base-url explicito (LEGACY, sube todo a ESE release), o --repo / el
    // repo RECORDADO (INCREMENTAL: assets al release acumulativo, manifest al de la version).
    let mut cfg = config::load();
    let legacy_base_url = flag(args, "--base-url").map(str::to_string);
    let (base_url, repo, set_version) = if let Some(b) = &legacy_base_url {
        (
            b.clone(),
            github::parse_release_base_url(b).map(|(o, r, _)| format!("{o}/{r}")),
            version.trim().to_string(),
        )
    } else {
        let repo_in = flag(args, "--repo")
            .map(str::to_string)
            .or_else(|| cfg.publish_repo.clone())
            .context("falta --repo o --base-url (o un repo recordado de un publish anterior)")?;
        let repo = github::normalize_repo(&repo_in)
            .with_context(|| format!("repo invalido: {repo_in:?} (usa usuario/repo)"))?;
        let tag = github::valid_tag(version).with_context(|| {
            format!("version/tag invalido: {version:?} (sin espacios, '/' ni caracteres raros; ej v1.2.3)")
        })?;
        // El base_url del manifest apunta al release ACUMULATIVO de assets (no al de la version): ahi
        // viven los assets content-addressed. El tag validado es el del release de la VERSION.
        (github::assets_base_url(&repo), Some(repo), tag)
    };
    let base_url = base_url.as_str();

    let mods = modlist::scan(install)?;
    let ids: BTreeSet<String> = match flag(args, "--profile") {
        Some(pn) => profile::list()
            .into_iter()
            .find(|p| p.name == pn)
            .with_context(|| format!("no existe el perfil {pn:?}"))?
            .enabled_ids
            .into_iter()
            .collect(),
        None => mods
            .iter()
            .filter(|m| m.enabled)
            .map(|m| m.id().to_string())
            .collect(),
    };
    if ids.is_empty() {
        bail!("no hay mods para publicar (habilita algunos o pasa --profile)");
    }

    // RECORDAR el repo + nombre: la proxima vez `publish` sin --repo/--base-url reusa este repo.
    if let Some(r) = &repo {
        cfg.publish_repo = Some(r.clone());
        cfg.publish_set_name = Some(name.to_string());
        let _ = config::save(&cfg);
    }

    for w in publish::warnings(&ids) {
        println!("[!] {w}");
    }
    println!(
        "Hasheando {} mods... (puede tardar con .pck grandes)",
        ids.len()
    );
    let params = publish::PublishParams {
        set_name: name.to_string(),
        set_version: set_version.clone(),
        base_url: base_url.to_string(),
        published_at: String::new(),
        baselib_version: None,
    };
    let mut prep = publish::prepare(&mods, &ids, &params)?;
    let out_dir = Path::new(out);

    // Delta intra-archivo: generar patches bsdiff contra la publicacion anterior en out_dir, asi
    // un amigo que ya tiene la version vieja baja solo el diff (no el `.pck` entero). Opcional.
    if args.iter().any(|a| a == "--no-delta") {
        println!("(--no-delta: no se generan patches incrementales)");
    } else {
        match publish::add_deltas(&mut prep, out_dir) {
            Ok(r) if r.patches > 0 => println!(
                "  deltas  : {} patch(es) ({:.1} MB) reemplazan {:.1} MB de full -> los amigos al dia bajan el diff",
                r.patches,
                r.patch_bytes as f64 / 1.0e6,
                r.full_bytes as f64 / 1.0e6,
            ),
            Ok(_) => {} // primera publicacion o nada cambio: sin deltas
            Err(e) => eprintln!("[!] no se pudieron generar deltas (se sigue sin ellos): {e:#}"),
        }
    }

    let manifest_path = publish::write_out(&prep, out_dir)?;

    println!("\nGenerado:");
    println!("  manifest: {}", manifest_path.display());
    println!(
        "  assets  : {} archivos ({:.1} MB) en {}/assets/",
        prep.assets.len(),
        prep.total_bytes() as f64 / 1.0e6,
        out_dir.display()
    );

    if args.iter().any(|a| a == "--no-upload") {
        println!("\nSubir a mano a un GitHub Release (gh CLI):");
        println!("  {}", publish::gh_hint(version, out_dir));
    } else {
        let via = if sts2_modsync::github::is_connected() {
            "API de GitHub (login en la app)"
        } else {
            "gh CLI (no hay login; usa `github-login <token>` para subir por API)"
        };
        println!("\nSubiendo al GitHub Release via {via}...");
        // Incremental por repo, salvo --base-url legacy (sube todo a ese release).
        let result = match (&legacy_base_url, &repo) {
            (Some(b), _) => publish::upload_to_release(out_dir, b),
            (None, Some(r)) => publish::upload(out_dir, r, &set_version),
            (None, None) => unreachable!("sin --base-url el repo siempre esta resuelto"),
        };
        match result {
            Ok(url) => println!("publicado: {url}"),
            Err(e) => {
                eprintln!("[!] no se pudo subir automaticamente: {e:#}");
                println!("Subi a mano:\n  {}", publish::gh_hint(version, out_dir));
            }
        }
    }
    // OJO: con el modelo incremental el `base_url` apunta al release de ASSETS, pero el MANIFEST vive
    // en el release de la VERSION. Para que el amigo no quede con una URL 404, recomendamos suscribirse
    // por REPO (sigue el ultimo release) y mostramos la URL del manifest de ESTA version.
    match (&legacy_base_url, &repo) {
        (None, Some(r)) => println!(
            "\nPasale a tus amigos el REPO (pestaña Sync -> Repositorio, sigue el ultimo release):\n  {r}\n  (o la URL de esta version: {}set-manifest.json)",
            github::release_base_url(r, &set_version)
        ),
        _ => println!(
            "\nPasale a tus amigos esta URL (pestaña Sync):\n  {}set-manifest.json",
            base_url.trim_end_matches('/').to_string() + "/"
        ),
    }
    Ok(())
}

/// `mod-source <id> <usuario/repo|URL>`: fija el origen (upstream) de un mod para auto-actualizarlo.
/// Se recuerda en `config.mod_sources` (prioridad sobre el `<id>.json`).
fn cmd_mod_source(args: &[String]) -> Result<()> {
    let id = arg(args, 1)?;
    let url = arg(args, 2)?;
    let src = ModSource::parse(url).with_context(|| {
        format!("origen invalido: {url:?} (usa usuario/repo o una URL de Nexus)")
    })?;
    let mut cfg = config::load();
    cfg.mod_sources.insert(id.to_string(), src.to_storage());
    config::save(&cfg)?;
    println!("origen de {id}: {}", src.label());
    Ok(())
}

/// Canal de un mod->update legible (estable/beta).
fn chan_label(prerelease: bool) -> &'static str {
    if prerelease { "beta" } else { "estable" }
}

/// `mod-check [<id>]`: chequea si hay version nueva (canal global estable/beta) de los mods con
/// origen GitHub configurado; con `<id>`, solo ese. Los de Nexus se listan con su pagina (fase 2).
fn cmd_mod_check(install: &detect::Install, args: &[String]) -> Result<()> {
    let cfg = config::load();
    let mods = modlist::scan(install)?;
    let only = args.get(1).map(String::as_str);
    let canal = if cfg.prefer_beta { "beta" } else { "estable" };
    println!("canal: {canal}");
    let mut found = 0usize;
    for m in mods.iter().filter(|m| only.is_none_or(|id| m.id() == id)) {
        let Some(src) = modupdate::effective_source(m, &cfg) else {
            continue;
        };
        match &src {
            ModSource::GitHub { owner, repo } => {
                match modupdate::check_github(
                    owner,
                    repo,
                    m.id(),
                    m.manifest.version.as_deref(),
                    cfg.mod_installed_tag.get(m.id()).map(String::as_str),
                    cfg.prefer_beta,
                ) {
                    Ok(Some(u)) => {
                        println!(
                            "  [{}] v{} -> v{} ({})  {}",
                            m.id(),
                            u.current.as_deref().unwrap_or("?"),
                            u.latest,
                            chan_label(u.prerelease),
                            u.html_url
                        );
                        found += 1;
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("  [{}] error: {e:#}", m.id()),
                }
            }
            ModSource::Nexus { game, mod_id } => {
                if !sts2_modsync::nexus::is_connected() {
                    println!(
                        "  [{}] Nexus ({}) — conecta con `nexus-login` para chequear",
                        m.id(),
                        src.label()
                    );
                    continue;
                }
                match sts2_modsync::nexus::check(game, *mod_id, m.manifest.version.as_deref()) {
                    Ok(Some(u)) => {
                        println!(
                            "  [{}] Nexus -> v{} — `mod-update {}` (Premium) o \"Mod Manager \
                             Download\" en {}",
                            m.id(),
                            u.latest,
                            m.id(),
                            src.web_url()
                        );
                        found += 1;
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("  [{}] Nexus error: {e:#}", m.id()),
                }
            }
        }
    }
    if found == 0 {
        println!("(sin actualizaciones, o ningun mod tiene origen GitHub configurado)");
    }
    Ok(())
}

/// `mod-update <id>`: chequea e INSTALA la version nueva del mod (reemplaza, preserva enable/disable).
fn cmd_mod_update(install: &detect::Install, args: &[String]) -> Result<()> {
    let id = arg(args, 1)?;
    let cfg = config::load();
    let mods = modlist::scan(install)?;
    let m = mods
        .iter()
        .find(|m| m.id() == id)
        .with_context(|| format!("el mod {id:?} no esta instalado"))?;
    let src = modupdate::effective_source(m, &cfg).with_context(|| {
        format!("{id} no tiene origen (usa: mod-source {id} <usuario/repo|URL>)")
    })?;
    let current = m.manifest.version.as_deref();
    let installed_tag = cfg.mod_installed_tag.get(id).map(String::as_str);
    let al_dia = || {
        format!(
            "{id} ya esta en la ultima version ({})",
            current.unwrap_or("?")
        )
    };
    match &src {
        ModSource::GitHub { owner, repo } => {
            let upd =
                modupdate::check_github(owner, repo, id, current, installed_tag, cfg.prefer_beta)?
                    .with_context(al_dia)?;
            println!(
                "actualizando {id}: v{} -> v{} ({})...",
                upd.current.as_deref().unwrap_or("?"),
                upd.latest,
                chan_label(upd.prerelease)
            );
            modupdate::apply(install, id, &upd.asset_url, &upd.tag)?;
            println!("listo: {id} v{}", upd.latest);
        }
        ModSource::Nexus { game, mod_id } => {
            // Premium: descarga DIRECTA (resuelve el archivo MAIN). Free: hay que usar el handler nxm://.
            let premium = sts2_modsync::nexus::validate()
                .map(|u| u.is_premium)
                .unwrap_or(false);
            let upd = modupdate::check_nexus(id, game, *mod_id, current, installed_tag, premium)?
                .with_context(al_dia)?;
            let Some(nref) = &upd.nexus else {
                bail!(
                    "{id} es de Nexus ({}) y tu cuenta no es Premium: usa \"Mod Manager Download\" \
                     en Nexus (handler nxm://) o conecta una cuenta Premium con nexus-login. Abri {}",
                    src.label(),
                    src.web_url()
                );
            };
            println!("actualizando {id} desde Nexus -> v{}...", upd.latest);
            modupdate::apply_nexus(install, id, nref, &upd.latest)?;
            println!("listo: {id} v{}", upd.latest);
        }
    }
    Ok(())
}

/// `github-login <token>` / `github-logout` / `github-status`: gestiona el token de GitHub
/// (guardado SEGURO en el llavero del SO) que usa `publish` para subir por la API sin el `gh` CLI.
fn cmd_github(cmd: &str, args: &[String]) -> Result<()> {
    use sts2_modsync::github;
    match cmd {
        "github-login" => {
            // El token NO se toma de argv (quedaria en el historial del shell / process list):
            // se lee de la env `GITHUB_TOKEN` o, si no, de stdin.
            let token = read_login_token(args)?;
            let token = token.trim();
            if token.is_empty() {
                bail!("token vacio");
            }
            let login = github::Api::new(token.to_string())
                .whoami()
                .context("token invalido o sin permiso")?;
            github::store_token(token)?;
            println!("conectado a GitHub como {login} (token guardado en el llavero del SO)");
        }
        "github-logout" => {
            github::clear_token()?;
            println!("desconectado (token borrado del llavero).");
        }
        "github-status" => match github::load_token() {
            Some(t) => match github::Api::new(t).whoami() {
                Ok(login) => println!("conectado como {login}."),
                Err(e) => println!("hay un token guardado pero NO valida: {e:#}"),
            },
            None => println!("no conectado (usa `github-login <token>`)."),
        },
        _ => unreachable!(),
    }
    Ok(())
}

/// Lee el token de login SIN que quede en el historial del shell / la lista de procesos:
/// de la env `GITHUB_TOKEN`, o (con aviso) de un argumento posicional, o de stdin.
fn read_login_token(args: &[String]) -> Result<String> {
    read_secret(args, "GITHUB_TOKEN", "token de GitHub")
}

/// Lee un secreto (token/API key) SIN que quede en el historial del shell / la lista de procesos:
/// de la env `env`, o (con aviso) de un argumento posicional, o de stdin.
fn read_secret(args: &[String], env: &str, label: &str) -> Result<String> {
    if let Ok(t) = std::env::var(env)
        && !t.trim().is_empty()
    {
        return Ok(t);
    }
    if let Some(t) = args.get(1) {
        eprintln!(
            "[aviso] pasar el {label} por argumento queda en el historial del shell; preferi la env {env} o stdin."
        );
        return Ok(t.clone());
    }
    use std::io::Write;
    eprint!("Pega el {label} y Enter: ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .with_context(|| format!("leyendo el {label} de stdin"))?;
    Ok(line)
}

/// `nexus-login` / `nexus-logout` / `nexus-status`: gestiona la API Key de Nexus (guardada SEGURO en
/// el llavero del SO) que usa el auto-update de mods para chequear versiones de mods de Nexus.
fn cmd_nexus(cmd: &str, args: &[String]) -> Result<()> {
    use sts2_modsync::nexus;
    match cmd {
        "nexus-login" => {
            let key = read_secret(args, "NEXUS_APIKEY", "API Key de Nexus")?;
            let key = key.trim();
            if key.is_empty() {
                bail!("API key vacia");
            }
            nexus::store_key(key)?;
            match nexus::validate() {
                Ok(u) => {
                    let prem = if u.is_premium { " (Premium)" } else { "" };
                    println!(
                        "conectado a Nexus como {}{prem} (key guardada en el llavero)",
                        u.name
                    );
                }
                Err(e) => {
                    let _ = nexus::clear_key(); // no dejar una key invalida guardada
                    return Err(e);
                }
            }
        }
        "nexus-logout" => {
            nexus::clear_key()?;
            println!("desconectado de Nexus (API key borrada del llavero).");
        }
        "nexus-status" => match nexus::is_connected() {
            true => match nexus::validate() {
                Ok(u) => println!(
                    "conectado como {}{}.",
                    u.name,
                    if u.is_premium { " (Premium)" } else { "" }
                ),
                Err(e) => println!("hay una API key guardada pero NO valida: {e:#}"),
            },
            false => println!("no conectado (usa `nexus-login`)."),
        },
        _ => unreachable!(),
    }
    Ok(())
}

/// Tope de la descarga de un archivo de Nexus (mods con `.pck` grandes pueden pesar varios GB).
const NXM_DOWNLOAD_MAX: u64 = 4 * 1024 * 1024 * 1024;

/// `nxm <link>` (lo invoca Windows al tocar "Mod Manager Download" en Nexus): resuelve el download-link,
/// baja e instala. `nxm-register` / `nxm-unregister`: alta/baja del handler del protocolo. Como se lanza
/// por el protocolo (sin consola), el resultado se muestra en un DIALOGO.
fn cmd_nxm(cmd: &str, args: &[String]) -> Result<()> {
    use sts2_modsync::nxm;
    match cmd {
        "nxm-register" => {
            nxm::register()?;
            println!("registrado como handler de nxm:// (Mod Manager Download de Nexus).");
            Ok(())
        }
        "nxm-unregister" => {
            nxm::unregister()?;
            println!("handler nxm:// removido.");
            Ok(())
        }
        "nxm" => {
            // Lanzado por el protocolo (sin consola): TODO feedback va por dialogo, incluido el link faltante.
            let Some(link) = args.get(1) else {
                nxm_dialog(
                    "Nexus — error",
                    "falta el link nxm:// (lo deberia pasar el navegador al tocar \"Mod Manager Download\").",
                    true,
                );
                bail!("uso: nxm <link>");
            };
            let res = run_nxm(link);
            match &res {
                Ok(msg) => {
                    println!("{msg}");
                    nxm_dialog("Nexus", msg, false);
                }
                Err(e) => {
                    let m = format!("{e:#}");
                    eprintln!("{m}");
                    nxm_dialog("Nexus — no se pudo instalar", &m, true);
                }
            }
            res.map(|_| ())
        }
        _ => unreachable!(),
    }
}

/// Baja+instala el archivo que apunta un link `nxm://`. Devuelve un mensaje para el dialogo.
fn run_nxm(link: &str) -> Result<String> {
    use sts2_modsync::{nexus, nxm};
    // NO interpolar el link crudo: trae `?key=..&expires=..` (credencial de un solo uso). Reportar
    // solo la parte SIN query (hasta el primer `?`), para no filtrarla al dialogo/stderr.
    let l = nxm::parse_nxm_link(link).with_context(|| {
        let safe = link.split('?').next().unwrap_or(link);
        format!("link nxm invalido: {safe:?} (esperado nxm://<game>/mods/<id>/files/<id>)")
    })?;
    let cfg = config::load();
    // Auto-deteccion SOLO (sin dialogo de carpeta: lanzado por el protocolo, un selector que aparece
    // de la nada confunde). Si no hay install, pedir abrir la app primero.
    let install = detect_install_auto(&cfg).context(
        "no se detecto Slay the Spire 2. Abri la app (sin argumentos) una vez para fijar la carpeta, \
         y reintenta desde Nexus.",
    )?;
    if detect::is_game_running() {
        // El link nxm es de UN SOLO USO y caduca: hay que re-iniciar desde la web tras cerrar el juego.
        bail!(
            "Cerra Slay the Spire 2 y volve a tocar \"Mod Manager Download\" en Nexus (el link es de \
             un solo uso y ya caduco)."
        );
    }
    println!(
        "Nexus {}/{} (archivo {}) — resolviendo la descarga...",
        l.game, l.mod_id, l.file_id
    );
    let url = nexus::download_link(
        &l.game,
        l.mod_id,
        l.file_id,
        l.key.as_deref(),
        l.expires.as_deref(),
    )?;
    // Nombre de temp con la extension de la URL (para que `move_to_downloads` lo guarde con un nombre
    // sensato si no es instalable). El FORMATO real (zip/7z) lo decide `archive_kind` por MAGIC.
    let tmp = std::env::temp_dir().join(format!(
        "sts2_nxm_{}.{}",
        sts2_modsync::util::unique_nanos(),
        url_extension(&url).as_deref().unwrap_or("bin")
    ));
    println!("bajando...");
    if let Err(e) = transport::download_capped(&url, &tmp, NXM_DOWNLOAD_MAX) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    use manager::ArchiveKind;
    match manager::archive_kind(&tmp) {
        // .zip o .7z: instalar. Si el id YA esta instalado, CONFIRMAR antes de reemplazar (este flujo
        // lo lanza el protocolo, no la app: sin el prompt pisaria el mod en silencio).
        ArchiveKind::Zip | ArchiveKind::SevenZ => {
            let r = manager::install_from_zip_confirmed(&install, &tmp, confirm_replace_dialog);
            let _ = std::fs::remove_file(&tmp);
            match r.context("instalando el archivo de Nexus")? {
                Some(id) => Ok(format!("instalado desde Nexus: {id}")),
                None => {
                    Ok("cancelado: el mod ya estaba instalado y elegiste no reemplazarlo.".into())
                }
            }
        }
        // .rar u otro: NO se extrae; preservar el archivo (a Descargas) para instalar a mano.
        ArchiveKind::Other => match move_to_downloads(&tmp, &url) {
            Ok(dst) => Ok(format!(
                "el archivo de Nexus no es .zip ni .7z. Lo guarde en {}; extraelo e instala con \
                 'Instalar carpeta' o 'Instalar .zip' (pestaña Mods).",
                dst.display()
            )),
            Err(_) => Ok(format!(
                "el archivo de Nexus no es .zip ni .7z y quedo en {}; extraelo e instala a mano.",
                tmp.display()
            )),
        },
    }
}

/// Deteccion del install SIN dialogo manual (cacheado + auto). Para `nxm` lanzado por el protocolo.
fn detect_install_auto(cfg: &config::Config) -> Option<detect::Install> {
    if let Some(root) = &cfg.install_root
        && let Some(i) = detect::from_root(root)
    {
        return Some(i);
    }
    detect::detect()
}

/// Extension (lowercase) del archivo de una URL: solo el ultimo segmento del path (sin query),
/// partiendo por `/` Y `\` para no dejar pasar separadores de Windows.
fn url_extension(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit(['/', '\\']).next().unwrap_or("");
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())
}

/// Nombre de archivo SEGURO derivado de una URL: solo el ultimo segmento, sin separadores (`/` `\`),
/// `:`, control-chars ni `.` al borde (cierra path-traversal si el CDN devolviera un nombre hostil).
fn safe_download_name(url: &str) -> String {
    let raw = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    let cleaned: String = raw
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':') && !c.is_control())
        .collect();
    let cleaned = cleaned.trim().trim_matches('.');
    if cleaned.is_empty() {
        "nexus_mod.bin".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Mueve el archivo bajado a la carpeta de Descargas (o temp) con un nombre SANEADO. rename si se
/// puede, sino copy+borrar. Devuelve el destino.
fn move_to_downloads(tmp: &Path, url: &str) -> Result<std::path::PathBuf> {
    let dir = directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir);
    let dst = dir.join(safe_download_name(url));
    if std::fs::rename(tmp, &dst).is_ok() {
        return Ok(dst);
    }
    std::fs::copy(tmp, &dst).with_context(|| format!("guardando en {}", dst.display()))?;
    let _ = std::fs::remove_file(tmp);
    Ok(dst)
}

/// Dialogo Si/No para confirmar reemplazar un mod YA instalado en el flujo `nxm://` (lo lanza el
/// protocolo, no la app). `true` = el usuario eligio reemplazar. Si el dialogo no se puede mostrar,
/// rfd devuelve un resultado distinto de `Yes` -> NO reemplaza (conservador: ante la duda, no pisar).
fn confirm_replace_dialog(id: &str) -> bool {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Reemplazar un mod ya instalado")
        .set_description(format!(
            "Ya tenes el mod \"{id}\" instalado. La descarga de Nexus lo va a REEMPLAZAR (la version \
             actual va a la papelera, es reversible). ¿Reemplazarlo?"
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

/// Muestra un dialogo del SO con el resultado del nxm (cuando se lanza por el protocolo no hay
/// consola). Best-effort: si no se puede mostrar, no pasa nada.
fn nxm_dialog(title: &str, body: &str, error: bool) {
    let level = if error {
        rfd::MessageLevel::Error
    } else {
        rfd::MessageLevel::Info
    };
    rfd::MessageDialog::new()
        .set_level(level)
        .set_title(title)
        .set_description(body)
        .show();
}

/// `update`: chequea el ultimo release en GitHub y, si hay una version nueva, se actualiza.
fn cmd_update() -> Result<()> {
    println!("version actual: {}", update::current_version());
    match update::check_latest()? {
        None => println!("no hay releases publicados todavia."),
        Some(rel) if update::is_newer(&rel.version, update::current_version()) => {
            println!("version nueva: {} disponible.", rel.tag);
            if !rel.notes.trim().is_empty() {
                println!(
                    "\n--- notas del release ---\n{}\n-------------------------",
                    rel.notes.trim()
                );
            }
            println!("bajando y reemplazando el ejecutable...");
            update::apply(&rel)?; // reemplaza + verifica arranque + relanza + exit; no retorna en exito
        }
        Some(rel) => println!("ya estas al dia ({}).", rel.tag),
    }
    Ok(())
}

/// Valor de un flag `--clave valor` en los args (None si falta).
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn print_install(install: &detect::Install) {
    println!("Install: {}", install.root.display());
    println!("  fuente : {:?}", install.source);
    println!("  version: {}", install.version.as_deref().unwrap_or("?"));
    println!("  mods/  : {}", install.mods_dir.display());
    if detect::is_game_running() {
        println!("  [!] El juego esta ABIERTO — cerralo antes de tocar los mods.");
    }
}

fn print_mod(m: &modlist::InstalledMod) {
    let gp = if m.manifest.affects_gameplay {
        " [gameplay]"
    } else {
        ""
    };
    println!(
        "  {:<28} {:<10} {:>9}  {}{}",
        m.id(),
        m.manifest.version.as_deref().unwrap_or("?"),
        human_size(m.size_bytes),
        m.manifest.display_name(),
        gp,
    );
}

fn human_size(bytes: u64) -> String {
    sts2_modsync::util::human_size(bytes, false)
}

fn print_plan(plan: &sync::Plan) {
    println!("  orden instalacion: {}", plan.install_order.join(" -> "));
    println!("  orden de carga   : {}", plan.load_order.join(" -> "));
    if !plan.load_order_enforced {
        println!(
            "  [!] falta ModListSorter en el set: sin el, los amigos pueden quedar con otro\n      orden de carga y no entrar al lobby (room-hash de BaseLib distinto)."
        );
    }
    println!("  al dia           : {} archivos", plan.up_to_date.len());
    println!(
        "  a descargar      : {} archivos ({:.1} MB)",
        plan.to_download.len(),
        plan.bytes_to_download as f64 / 1.0e6
    );
    for d in &plan.to_download {
        let tag = if d.is_delta() {
            format!(" [delta {} B]", d.fetch_bytes())
        } else {
            String::new()
        };
        println!("    + {}  ({} B){tag}", d.entry.path, d.entry.size);
    }
    if !plan.orphans.is_empty() {
        println!("  huerfanos a borrar: {}", plan.orphans.len());
        for o in &plan.orphans {
            println!("    - {}", o.display());
        }
    }
    if plan.is_noop() {
        println!("  => todo sincronizado, nada que hacer.");
    }
    println!("\n(`sync` por CLI es dry-run; para instalar de verdad usa la pestaña Sync del GUI.)");
}

fn print_help() {
    println!("sts2-modsync — mod manager + sync para Slay the Spire 2\n");
    println!("Uso: sts2-modsync <subcomando>");
    println!("  list                  lista los mods instalados (default)");
    println!("  enable  <id>          habilita un mod (mueve la carpeta a mods/)");
    println!("  disable <id>          deshabilita un mod (a mods_disabled/)");
    println!("  launch [--direct]     lanza el juego (por Steam; --direct abre el exe sin Steam)");
    println!(
        "  dedupe                limpia mods duplicados (deja la version mas nueva, resto a papelera)"
    );
    println!(
        "  loadcode [<codigo>]   sin arg: imprime el codigo de la lista activa; con codigo: lo aplica"
    );
    println!(
        "  sync <set.json|url|owner/repo>  dry-run del plan; con owner/repo sigue el ultimo release"
    );
    println!(
        "  publish --name <s> --version <v> [--repo <owner/repo> | --base-url <url>] [--profile <p>] [--out <dir>]"
    );
    println!(
        "                        genera + sube un set desde tus mods (modder); el --repo se RECUERDA"
    );
    println!(
        "                        (la proxima vez podes omitirlo: sube OTRO release al mismo repo)"
    );
    println!(
        "  mod-source <id> <usuario/repo|URL>   fija el origen de un mod (GitHub/Nexus) para auto-update"
    );
    println!("  mod-check [<id>]      busca version nueva de los mods (canal global estable/beta)");
    println!("  mod-update <id>       baja e instala la version nueva de un mod (origen GitHub)");
    println!("  update                chequea GitHub y actualiza la app si hay version nueva");
    println!("  keygen                genera el par de claves minisign del modder (firma sets)");
    println!(
        "  sign    <archivo>     firma un archivo (.minisig); clave de MINISIGN_SECRET_KEY o keygen"
    );
    println!(
        "  seed    <out_dir>     seedea por P2P (torrent) un set publicado (necesita --features p2p)"
    );
    println!(
        "  github-login          guarda un token de GitHub en el llavero (lee de GITHUB_TOKEN o stdin); publish sube por API (sin gh)"
    );
    println!("  github-status         muestra si estas conectado a GitHub");
    println!("  github-logout         borra el token guardado");
    println!(
        "  nexus-login           guarda tu API Key de Nexus en el llavero (lee de NEXUS_APIKEY o stdin); chequea versiones de mods de Nexus"
    );
    println!("  nexus-status / nexus-logout   estado / desconectar de Nexus");
    println!(
        "  nxm-register / nxm-unregister registra/saca esta app como handler de nxm:// (\"Mod Manager Download\" de Nexus)"
    );
    println!(
        "  nxm <link>            (lo invoca Windows) baja+instala el archivo de un link nxm:// de Nexus"
    );
    println!(
        "\nGUI (mod manager con pestañas): corre el exe SIN argumentos (o `cargo run --features gui`)."
    );
}

#[cfg(test)]
mod tests {
    use super::{safe_download_name, url_extension};

    #[test]
    fn safe_download_name_cierra_path_traversal() {
        // nombre normal: se conserva.
        assert_eq!(
            safe_download_name("https://cdn/files/MiMod-1.2.zip?md5=x&expires=9"),
            "MiMod-1.2.zip"
        );
        // backslashes de Windows en el ultimo segmento: NO sobreviven (no escapan de Descargas).
        let bad = safe_download_name("https://cdn/a/..\\..\\Startup\\evil.bat");
        assert!(
            !bad.contains('\\') && !bad.contains('/'),
            "no debe traer separadores: {bad}"
        );
        // ".." y nombre vacio -> fallback seguro.
        assert_eq!(safe_download_name("https://cdn/x/.."), "nexus_mod.bin");
        assert_eq!(safe_download_name("https://cdn/"), "nexus_mod.bin");
    }

    #[test]
    fn url_extension_parte_por_ambos_separadores() {
        assert_eq!(
            url_extension("https://cdn/a/b.ZIP?x=1").as_deref(),
            Some("zip")
        );
        assert_eq!(url_extension("https://cdn/a/b.7z").as_deref(), Some("7z"));
        assert_eq!(url_extension("https://cdn/noext").as_deref(), None);
    }
}
