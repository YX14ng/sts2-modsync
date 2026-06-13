//! sts2-modsync — MVP de linea de comandos (FASE 1).
//!
//! Hoy demuestra el CORE que ya funciona: detecta el install de Slay the Spire 2
//! (Steam o, si falla, dialogo de carpeta para copias pirata), lee un set-manifest
//! local, y muestra el PLAN de sincronizacion (dry-run: que bajaria, que ya esta,
//! que sobra). La DESCARGA real + la GUI (eframe) + el delta del .pck son FASE 2,
//! a cargo del proximo Claude Code — ver HANDOFF.md.
//!
//! Uso:
//!   sts2-modsync                     -> detecta el install y lo reporta
//!   sts2-modsync <manifiesto.json>   -> ademas calcula el plan contra ese set

use anyhow::Result;
use std::path::Path;
use sts2_modsync::{config, detect, manifest::SetManifest, sync};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let cfg = config::load();
    let Some(install) = resolve_install(&cfg) else {
        eprintln!("No se encontro Slay the Spire 2 y no se eligio carpeta. Abortando.");
        std::process::exit(1);
    };

    println!("Install: {}", install.root.display());
    println!("  fuente : {:?}", install.source);
    println!("  version: {}", install.version.as_deref().unwrap_or("?"));
    println!("  mods/  : {}", install.mods_dir.display());
    if detect::is_game_running() {
        println!("  [!] El juego esta ABIERTO — cerralo antes de sincronizar (lock de .dll/.pck).");
    }

    // Cachear la ruta hallada para la proxima.
    if cfg.install_root.as_deref() != Some(install.root.as_path()) {
        let mut cfg = cfg;
        cfg.install_root = Some(install.root.clone());
        let _ = config::save(&cfg);
    }

    match args.first() {
        Some(manifest_path) => {
            let manifest = SetManifest::from_json_file(Path::new(manifest_path))?;
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
        }
        None => println!("\n(Pasa un manifiesto.json como argumento para ver el plan de sync.)"),
    }
    Ok(())
}

/// config cacheada (re-validada) -> deteccion automatica -> dialogo manual.
fn resolve_install(cfg: &config::Config) -> Option<detect::Install> {
    if let Some(root) = &cfg.install_root {
        if let Some(i) = detect::from_root(root) {
            return Some(i);
        }
    }
    detect::detect().or_else(detect::pick_folder_dialog)
}

fn print_plan(plan: &sync::Plan) {
    println!("  orden instalacion: {}", plan.install_order.join(" -> "));
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
    println!("\n(La descarga/apply real es FASE 2 — ver HANDOFF.md.)");
}
