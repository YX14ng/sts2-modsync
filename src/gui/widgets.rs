//! Widgets y helpers de presentacion reusables del GUI: la "card" enmarcada, el item de
//! navegacion del sidebar, formateo legible (MB/velocidad/ETA), el hint accionable de los
//! toasts, el filtro de mods y el onboarding del orden de carga. Sin estado propio (free fns).

use super::ACCENT;
use crate::modlist::InstalledMod;
use eframe::egui;

/// Notificacion efimera (toast). Los exitos se auto-descartan a los pocos segundos; los
/// errores quedan hasta que el usuario los cierra y muestran un hint accionable.
pub(super) struct Toast {
    pub(super) msg: String,
    pub(super) is_error: bool,
    pub(super) at: std::time::Instant,
}

/// Seccion enmarcada ("card") reusable para darle jerarquia al contenido.
pub(super) fn card<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::default()
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(8)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            if !title.is_empty() {
                ui.label(egui::RichText::new(title).strong().color(ACCENT));
                ui.add_space(6.0);
            }
            add(ui)
        })
        .inner
}

/// Item de navegacion full-width del sidebar.
pub(super) fn nav_item(ui: &mut egui::Ui, selected: bool, label: &str) -> bool {
    let w = ui.available_width();
    ui.add_sized([w, 32.0], egui::Button::selectable(selected, label))
        .clicked()
}

/// Bytes -> "X.Y MB" (las descargas de mods son escala MB).
pub(super) fn human_mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

/// Velocidad legible (B/s, KB/s, MB/s).
pub(super) fn human_speed(bps: f64) -> String {
    if bps >= 1_048_576.0 {
        format!("{:.1} MB/s", bps / 1_048_576.0)
    } else if bps >= 1024.0 {
        format!("{:.0} KB/s", bps / 1024.0)
    } else {
        format!("{:.0} B/s", bps.max(0.0))
    }
}

/// Segundos restantes -> "Xm Ys" / "Xs" / "—" si no se puede estimar.
pub(super) fn human_eta(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return "—".into();
    }
    let s = secs.round() as u64;
    if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// Hint accionable para un mensaje de error (heuristica por palabras clave).
pub(super) fn toast_hint(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if m.contains("abierto") || m.contains("juego") {
        "Cerra Slay the Spire 2 y reintenta (lockea sus archivos mientras corre)."
    } else if m.contains("espacio") {
        "Libera espacio en disco y reintenta."
    } else if m.contains("denegad")
        || m.contains("permis")
        || m.contains("os error 5")
        || m.contains("access is denied")
    {
        "Permiso denegado: cerra el juego; si sigue, la carpeta del juego no es escribible \
         (probá mover la libreria de Steam, o excluila del \"Controlled Folder Access\" de Defender)."
    } else if m.contains("firma") {
        "El set no esta firmado por el publicador de confianza; no lo instales si no sabes su origen."
    } else if m.contains("cancel") {
        "Cancelado; los .part quedan para reanudar cuando quieras."
    } else if m.contains("http") || m.contains("url") || m.contains("descarg") || m.contains("red")
    {
        "Revisa la URL del set y tu conexion a internet."
    } else {
        "Reintenta; si persiste, mira el log en %APPDATA%/sts2-modsync/sts2-modsync.log."
    }
}

pub(super) fn mod_matches(m: &InstalledMod, filter_lower: &str) -> bool {
    m.id().to_ascii_lowercase().contains(filter_lower)
        || m.manifest
            .display_name()
            .to_ascii_lowercase()
            .contains(filter_lower)
        || m.manifest
            .author
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains(filter_lower)
}

pub(super) fn human_size(bytes: u64) -> String {
    crate::util::human_size(bytes, true)
}

/// Explicacion (colapsable) de BaseLib / ModListSorter / orden de carga para no-tecnicos.
/// Onboarding: aparece donde se muestra el orden de carga (multiplayer).
pub(super) fn onboarding_load_order(ui: &mut egui::Ui) {
    ui.collapsing("¿Que es el orden de carga? (multiplayer)", |ui| {
        ui.label(
            egui::RichText::new(
                "En multiplayer el juego calcula un 'room-hash' a partir del ORDEN en que carga \
                 los mods. Si vos y un amigo cargan en distinto orden, el hash difiere y NO entran \
                 al mismo lobby.",
            )
            .weak(),
        );
        ui.label(
            egui::RichText::new(
                "BaseLib es la libreria base (carga primero). ModListSorter fuerza el orden \
                 canonico (BaseLib + A-Z) al cerrar el juego, asi todos convergen al mismo. Por \
                 eso un set para jugar juntos DEBE incluir ambos.",
            )
            .weak(),
        );
    });
}
