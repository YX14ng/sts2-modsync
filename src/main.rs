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
    config, detect, launch, manager, manifest::SetManifest, modlist, profile, publish, signing,
    sync, transport, update,
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
            launch::launch(&install)?;
            println!("lanzando Slay the Spire 2...");
        }
        "publish" => cmd_publish(&install, &args)?,
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
                "subcomando desconocido: {other:?} (probá: list|enable|disable|launch|sync|seed|help)"
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

    // Resolver base_url + repo: --base-url explicito, o --repo, o el repo RECORDADO de antes.
    let mut cfg = config::load();
    let (base_url, repo, set_version) = if let Some(b) = flag(args, "--base-url") {
        (
            b.to_string(),
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
        // El tag validado va a base_url Y a set_version (no la version cruda: evita que whitespace/
        // CRLF de los extremos termine en el set_version del manifest FIRMADO).
        (github::release_base_url(&repo, &tag), Some(repo), tag)
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
        set_version,
        base_url: base_url.to_string(),
        published_at: String::new(),
        baselib_version: None,
    };
    let prep = publish::prepare(&mods, &ids, &params)?;
    let out_dir = Path::new(out);
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
        match publish::upload(out_dir, base_url) {
            Ok(url) => println!("publicado: {url}"),
            Err(e) => {
                eprintln!("[!] no se pudo subir automaticamente: {e:#}");
                println!("Subi a mano:\n  {}", publish::gh_hint(version, out_dir));
            }
        }
    }
    println!(
        "\nPasale a tus amigos esta URL (pestaña Sync):\n  {}set-manifest.json",
        base_url.trim_end_matches('/').to_string() + "/"
    );
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
    if let Ok(t) = std::env::var("GITHUB_TOKEN")
        && !t.trim().is_empty()
    {
        return Ok(t);
    }
    if let Some(t) = args.get(1) {
        eprintln!(
            "[aviso] pasar el token por argumento queda en el historial del shell; preferi la env GITHUB_TOKEN o stdin."
        );
        return Ok(t.clone());
    }
    use std::io::Write;
    eprint!("Pega el token de GitHub y Enter: ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("leyendo el token de stdin")?;
    Ok(line)
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
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0}KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes}B")
    }
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
    for f in &plan.to_download {
        println!("    + {}  ({} B)", f.path, f.size);
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
    println!("  launch                lanza el juego");
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
        "\nGUI (mod manager con pestañas): corre el exe SIN argumentos (o `cargo run --features gui`)."
    );
}
