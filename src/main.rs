//! sts2-modsync — CLI del mod manager (+ sync) de Slay the Spire 2.
//!
//! Subcomandos:
//!   sts2-modsync [list]            lista los mods instalados (habilitados/deshabilitados)
//!   sts2-modsync enable  <id>      habilita un mod (mueve la carpeta a mods/)
//!   sts2-modsync disable <id>      deshabilita un mod (mueve la carpeta a mods_disabled/)
//!   sts2-modsync launch            lanza el juego
//!   sts2-modsync sync    <set.json> dry-run del plan de sincronizacion de un set
//!
//! La GUI (mod manager con pestañas) es `cargo run --features gui --bin sts2-modsync-gui`.

use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::path::Path;
use sts2_modsync::{
    config, detect, launch, manager, manifest::SetManifest, modlist, profile, publish, signing,
    sync, transport, update,
};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("list");

    if matches!(cmd, "help" | "-h" | "--help") {
        print_help();
        return Ok(());
    }
    if cmd == "update" {
        return cmd_update();
    }
    if cmd == "keygen" {
        return cmd_keygen();
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
            bail!("subcomando desconocido: {other:?} (probá: list|enable|disable|launch|sync|help)")
        }
    }
    Ok(())
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

fn cmd_sync(install: &detect::Install, src: &str) -> Result<()> {
    print_install(install);
    let (text, sig) = if src.starts_with("http") {
        let t = transport::get_text(src)?;
        let s = transport::get_text(&format!("{src}.minisig")).ok(); // firma opcional
        (t, s)
    } else {
        let t = std::fs::read_to_string(src)?;
        let s = std::fs::read_to_string(format!("{src}.minisig")).ok();
        (t, s)
    };
    signing::verify_with_embedded(text.as_bytes(), sig.as_deref())?;
    let manifest = SetManifest::from_json_str(&text)?;
    println!(
        "\nSet: {} v{}  ({} mods)",
        manifest.set_name,
        manifest.set_version,
        manifest.mods.len()
    );
    if let Some(bl) = &manifest.baselib_version {
        println!("  BaseLib esperada: {bl}");
    }
    let plan = sync::plan(&manifest, &install.mods_dir)?;
    print_plan(&plan);
    Ok(())
}

/// `publish --name <set> --version <ver> --base-url <url> [--profile <p>] [--out <dir>]`
fn cmd_publish(install: &detect::Install, args: &[String]) -> Result<()> {
    let name = flag(args, "--name").context("falta --name")?;
    let version = flag(args, "--version").context("falta --version")?;
    let base_url = flag(args, "--base-url").context("falta --base-url")?;
    let out = flag(args, "--out").unwrap_or("./set-publish");

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

    for w in publish::warnings(&ids) {
        println!("[!] {w}");
    }
    println!(
        "Hasheando {} mods... (puede tardar con .pck grandes)",
        ids.len()
    );
    let params = publish::PublishParams {
        set_name: name.to_string(),
        set_version: version.to_string(),
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
    println!("\nSubir a un GitHub Release (gh CLI):");
    println!("  {}", publish::gh_hint(version, out_dir));
    println!("\nLuego pasale a tus amigos la URL del set-manifest.json (o el archivo).");
    Ok(())
}

/// `update`: chequea el ultimo release en GitHub y, si hay una version nueva, se actualiza.
fn cmd_update() -> Result<()> {
    println!("version actual: {}", update::current_version());
    match update::check_latest()? {
        None => println!("no hay releases publicados todavia."),
        Some(rel) if update::is_newer(&rel.version, update::current_version()) => {
            println!(
                "version nueva: {} — bajando y reemplazando el ejecutable...",
                rel.tag
            );
            update::apply(&rel)?; // reemplaza + relanza + exit; no retorna en exito
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
    println!("  sync    <set.json>    dry-run del plan de sincronizacion de un set");
    println!("  publish --name <s> --version <v> --base-url <url> [--profile <p>] [--out <dir>]");
    println!("                        genera un set-manifest + assets desde tus mods (modder)");
    println!("  update                chequea GitHub y actualiza la app si hay version nueva");
    println!("  keygen                genera el par de claves minisign del modder (firma sets)");
    println!("\nGUI (mod manager con pestañas): cargo run --features gui --bin sts2-modsync-gui");
}
