//! Reporte de DIAGNOSTICO (solo lectura) para pegar cuando "no podemos jugar juntos": junta en un
//! bloque el estado que importa para el multiplayer (version, install, ModListSorter, HUELLA de orden
//! de carga, mods habilitados + versiones, suscripciones, conflictos). Pura agregacion sobre
//! `detect`/`modlist`/`config` — no toca nada. Lo usan el CLI (`doctor`) y el GUI ("Copiar diagnostico").

use crate::config::Config;
use crate::detect::Install;
use crate::modlist::{self, InstalledMod};
use std::fmt::Write;

/// Arma el bloque de diagnostico (texto plano, para copiar/pegar en un chat de soporte).
pub fn report(install: &Install, mods: &[InstalledMod], cfg: &Config) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "sts2-modsync v{}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        s,
        "juego: {} ({:?}) v{}",
        install.root.display(),
        install.source,
        install.version.as_deref().unwrap_or("?")
    );
    let enabled = mods.iter().filter(|m| m.enabled).count();
    let _ = writeln!(s, "mods: {enabled} habilitados / {} instalados", mods.len());
    let _ = writeln!(
        s,
        "ModListSorter: {}",
        if modlist::load_order_enforced(mods) {
            "SI (orden de carga fijado)"
        } else {
            "NO  <-- riesgo: el orden puede divergir entre amigos (room-hash distinto)"
        }
    );
    let _ = writeln!(
        s,
        "huella de orden de carga: {}  (si coincide con tus amigos, entran al mismo lobby)",
        modlist::current_fingerprint(mods)
    );
    let conflicts = modlist::conflicts(mods);
    if !conflicts.is_empty() {
        let _ = writeln!(s, "conflictos (ids duplicados): {}", conflicts.join(", "));
    }
    let _ = writeln!(s, "orden de carga (habilitados):");
    for id in modlist::load_order(mods) {
        let ver = mods
            .iter()
            .find(|m| m.enabled && m.id() == id)
            .and_then(|m| m.manifest.version.as_deref())
            .unwrap_or("?");
        let _ = writeln!(s, "  {id} v{ver}");
    }
    if !cfg.subscribed_sets.is_empty() {
        let _ = writeln!(s, "sets suscriptos:");
        for k in &cfg.subscribed_sets {
            let v = cfg.set_versions.get(k).map(String::as_str).unwrap_or("?");
            let _ = writeln!(s, "  {k}  (v{v})");
        }
    }
    s
}
