//! GUI del mod manager (eframe/egui, single-exe, backend glow). Pestañas:
//! **Mods** (gestor: lista/detalle/on-off/instalar/desinstalar) · **Sync** (el añadido:
//! cargar un set-manifest, revisar el plan, aplicar) · **Perfiles** (conjuntos de mods
//! habilitados). Es una cascara sobre el core (detect/modlist/manager/profile/sync): NO
//! duplica logica. Todo el trabajo pesado (scan, mover/copiar carpetas, hashing) corre en
//! hilos aparte y se comunica por canales `mpsc`; la UI sondea en `ui()` (eframe 0.34) y
//! pide `ctx.request_repaint()`. NUNCA se bloquea el loop de egui. enable/disable mueven
//! carpetas (NO tocan `setting.save`); el orden lo impone ModListSorter.
//!
//! Este modulo se parte en submodulos por pestaña: `widgets` (helpers de presentacion),
//! `mods_tab`, `profiles_tab`, `publish_tab`, `github_login` y `sync_tab`. `mod.rs` conserva
//! el chasis (tema, ventana, topbar/nav, struct App y sus metodos transversales).

mod github_login;
mod mods_tab;
mod profiles_tab;
mod publish_tab;
mod sync_tab;
mod widgets;

use crate::detect::{self, Install};
use crate::modlist::{self, InstalledMod};
use crate::profile::Profile;
use crate::{config, launch, update};
use eframe::egui;
use github_login::GhEvent;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use sync_tab::SyncState;
use widgets::{Toast, nav_item, toast_hint};

const WARN: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x6C, 0x00);
const OK: egui::Color32 = egui::Color32::from_rgb(0x3F, 0xB9, 0x50);
const BAD: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x57, 0x57);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x7C, 0x9C, 0xFF);

/// Tema moderno: oscuro/claro con acento, espaciado generoso y tipografia mas grande.
/// El default de egui se ve "CMD"; esto le da jerarquia visual.
fn apply_theme(ctx: &egui::Context, dark: bool) {
    let mut style = (*ctx.global_style()).clone();
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.selection.bg_fill = ACCENT.linear_multiply(0.45);
    v.hyperlink_color = ACCENT;
    if dark {
        v.panel_fill = egui::Color32::from_rgb(0x17, 0x19, 0x21);
        v.window_fill = egui::Color32::from_rgb(0x1D, 0x20, 0x2A);
        v.extreme_bg_color = egui::Color32::from_rgb(0x10, 0x12, 0x18);
        v.faint_bg_color = egui::Color32::from_rgb(0x23, 0x26, 0x32);
        v.override_text_color = Some(egui::Color32::from_rgb(0xDA, 0xDE, 0xE8));
    }
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.window_margin = egui::Margin::same(10);
    use egui::{FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Heading, FontId::proportional(22.0)),
        (TextStyle::Body, FontId::proportional(15.0)),
        (TextStyle::Button, FontId::proportional(15.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
        (TextStyle::Small, FontId::proportional(12.0)),
    ]
    .into();
    ctx.set_global_style(style);
}

pub fn run() -> eframe::Result {
    // Log + panic-hook a %APPDATA% (el GUI puede no tener consola; un crash debe dejar rastro).
    crate::logging::init("gui");
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([920.0, 660.0])
            .with_min_inner_size([700.0, 480.0])
            .with_title("sts2-modsync — mod manager"),
        ..Default::default()
    };
    eframe::run_native(
        "sts2-modsync",
        options,
        Box::new(|cc| {
            install_cjk_font(&cc.egui_ctx);
            apply_theme(&cc.egui_ctx, true);
            Ok(Box::new(App::new()))
        }),
    )
}

/// Carga una fuente CJK del sistema como fallback (muchos mods tienen nombre/autor en
/// chino). Graceful: si no encuentra ninguna parseable, sigue sin CJK (cuadraditos).
fn install_cjk_font(ctx: &egui::Context) {
    let candidates = [
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\simsun.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\YuGothM.ttc",
    ];
    for path in candidates {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "cjk".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(fam)
                .or_default()
                .push("cjk".to_owned());
        }
        ctx.set_fonts(fonts);
        return;
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Mods,
    Sync,
    Profiles,
    Publish,
}

struct App {
    tab: Tab,
    cfg: config::Config,
    install: Option<Install>,
    install_note: String,
    game_running: bool,

    // Pestaña Mods
    mods: Vec<InstalledMod>,
    mods_loaded: bool,
    scan_job: Option<Receiver<Result<Vec<InstalledMod>, String>>>,
    filter: String,
    sort_enabled_first: bool, // orden de la lista: habilitados arriba (sino, solo alfabetico)
    selected: Option<String>,
    confirm_uninstall: Option<String>,

    // Accion en curso (enable/disable/install/uninstall/aplicar perfil): una a la vez.
    action_job: Option<Receiver<Result<String, String>>>,
    busy: String,
    toast: Option<Toast>,

    // Pestaña Sync
    sync: SyncState,

    // Pestaña Perfiles
    profiles: Vec<Profile>,
    profiles_loaded: bool,
    new_profile: String,

    // Pestaña Publicar
    pub_name: String,
    pub_version: String,
    pub_repo: String, // "owner/repo" (recordado en config para no recrear repos)
    pub_profile: Option<String>, // None = mods habilitados actuales
    pub_out_dir: Option<PathBuf>,
    // Seeding P2P del set publicado: Some(flag) mientras seedea (set flag=true para cortar).
    seed_stop: Option<Arc<AtomicBool>>,
    seed_status: Arc<Mutex<String>>,

    // GitHub (publicar sin `gh`): login con PAT o device-flow + estado.
    gh_user: Option<String>, // login conectado (None = no conectado / sin chequear)
    gh_user_checked: bool,   // ya validamos el token guardado
    gh_pat: String,          // input del token pegado
    gh_job: Option<Receiver<GhEvent>>, // worker de login (PAT o device-flow)
    gh_device: Option<(String, String)>, // (user_code, verification_uri) durante el device-flow

    // Auto-update
    update_checked: bool,
    update_check_job: Option<Receiver<Option<update::Release>>>,
    update_avail: Option<update::Release>,

    // Sets suscritos: chequeo manual de "version nueva" (worker que baja cada manifest).
    // El worker devuelve (updates clave->version_nueva, cantidad de sets que NO se pudieron chequear).
    set_check_job: Option<Receiver<(std::collections::HashMap<String, String>, usize)>>,
    set_updates: std::collections::HashMap<String, String>, // clave de sub -> version remota mas nueva

    dark_mode: bool,
}

impl App {
    fn new() -> Self {
        let cfg = config::load();
        // Pre-cargar el form de Publicar con lo ultimo recordado (repo + nombre del set).
        let pub_repo = cfg.publish_repo.clone().unwrap_or_default();
        let pub_name = cfg.publish_set_name.clone().unwrap_or_default();
        let mut app = App {
            tab: Tab::Mods,
            cfg,
            install: None,
            install_note: String::new(),
            game_running: false,
            mods: Vec::new(),
            mods_loaded: false,
            scan_job: None,
            filter: String::new(),
            sort_enabled_first: false,
            selected: None,
            confirm_uninstall: None,
            action_job: None,
            busy: String::new(),
            toast: None,
            sync: SyncState::default(),
            profiles: Vec::new(),
            profiles_loaded: false,
            new_profile: String::new(),
            pub_name,
            pub_version: String::new(),
            pub_repo,
            pub_profile: None,
            pub_out_dir: None,
            seed_stop: None,
            seed_status: Arc::new(Mutex::new(String::new())),
            gh_user: None,
            gh_user_checked: false,
            gh_pat: String::new(),
            gh_job: None,
            gh_device: None,
            update_checked: false,
            update_check_job: None,
            update_avail: None,
            set_check_job: None,
            set_updates: std::collections::HashMap::new(),
            dark_mode: true,
        };
        app.try_detect();
        app
    }

    // --- auto-update --------------------------------------------------------

    fn kick_update_check(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        self.update_check_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let res = update::check(); // best-effort: Option<Release>
            let _ = tx.send(res);
            ctx.request_repaint();
        });
    }

    fn poll_update_check(&mut self) {
        let Some(rx) = &self.update_check_job else {
            return;
        };
        match rx.try_recv() {
            Ok(res) => {
                self.update_avail = res;
                self.update_check_job = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.update_check_job = None,
        }
    }

    fn start_update(&mut self, ctx: &egui::Context, rel: update::Release) {
        self.run_action(ctx, format!("actualizando a {}...", rel.tag), move || {
            update::apply(&rel)?; // reemplaza + relanza + exit; en exito no retorna
            Ok("actualizado".into())
        });
    }

    // --- install (header) ---------------------------------------------------

    fn try_detect(&mut self) {
        let cached = self
            .cfg
            .install_root
            .as_ref()
            .and_then(|r| detect::from_root(r));
        match cached.or_else(detect::detect) {
            Some(i) => self.accept_install(i),
            None => self.install_note = "No se detecto. Elegi la carpeta del juego.".into(),
        }
    }

    fn accept_install(&mut self, i: Install) {
        self.game_running = detect::is_game_running();
        if self.cfg.install_root.as_deref() != Some(i.root.as_path()) {
            self.cfg.install_root = Some(i.root.clone());
            let _ = config::save(&self.cfg);
        }
        self.install_note.clear();
        self.install = Some(i);
        self.mods_loaded = false; // re-escanear
    }

    // --- jobs: scan + acciones ----------------------------------------------

    fn kick_scan(&mut self, ctx: &egui::Context) {
        let Some(install) = self.install.clone() else {
            return;
        };
        let (tx, rx) = channel();
        self.scan_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let res = modlist::scan(&install).map_err(|e| format!("{e:#}"));
            let _ = tx.send(res);
            ctx.request_repaint();
        });
    }

    fn poll_scan(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.scan_job else { return };
        match rx.try_recv() {
            Ok(Ok(mods)) => {
                self.mods = mods;
                self.mods_loaded = true;
                self.scan_job = None;
                self.game_running = detect::is_game_running();
            }
            Ok(Err(e)) => {
                self.show_toast(e, true);
                self.mods_loaded = true;
                self.scan_job = None;
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => self.scan_job = None,
        }
    }

    /// Muestra un toast (con timestamp para el auto-dismiss).
    fn show_toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some(Toast {
            msg: msg.into(),
            is_error,
            at: std::time::Instant::now(),
        });
    }

    /// Renderiza el toast actual: los exitos se auto-descartan a los 4 s; los errores quedan
    /// con un boton de cierre y un hint accionable. Llamar en cada pestaña que quiera mostrarlo.
    fn render_toast(&mut self, ui: &mut egui::Ui) {
        let Some(t) = &self.toast else {
            return;
        };
        if !t.is_error && t.at.elapsed() > std::time::Duration::from_secs(4) {
            self.toast = None;
            return;
        }
        let (msg, is_error) = (t.msg.clone(), t.is_error);
        let mut dismiss = false;
        ui.horizontal(|ui| {
            ui.colored_label(if is_error { BAD } else { OK }, &msg);
            if ui.small_button("✕").clicked() {
                dismiss = true;
            }
        });
        if is_error {
            ui.label(egui::RichText::new(toast_hint(&msg)).italics().weak());
        } else {
            // mantener el repaint para que el auto-dismiss ocurra aunque no haya input.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
        }
        if dismiss {
            self.toast = None;
        }
    }

    /// Corre una accion del manager en un hilo (una a la vez). Al terminar, re-escanea.
    fn run_action(
        &mut self,
        ctx: &egui::Context,
        busy: String,
        f: impl FnOnce() -> anyhow::Result<String> + Send + 'static,
    ) {
        if self.action_job.is_some() {
            return;
        }
        self.busy = busy;
        self.toast = None;
        let (tx, rx) = channel();
        self.action_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let res = f().map_err(|e| format!("{e:#}"));
            let _ = tx.send(res);
            ctx.request_repaint();
        });
    }

    fn poll_action(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.action_job else { return };
        match rx.try_recv() {
            Ok(res) => {
                self.action_job = None;
                self.busy.clear();
                match res {
                    Ok(m) => self.show_toast(m, false),
                    Err(e) => self.show_toast(e, true),
                }
                self.mods_loaded = false; // refrescar lista
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => {
                self.action_job = None;
                self.busy.clear();
                self.mods_loaded = false;
                self.show_toast("la operacion no se completo (worker terminado)", true);
            }
        }
    }

    /// True si hay CUALQUIER trabajo de fondo en curso (scan, accion, plan, fetch, apply).
    /// Usado para no disparar acciones destructivas (update, cargar set) en paralelo.
    fn any_job(&self) -> bool {
        !self.busy.is_empty()
            || self.action_job.is_some()
            || self.scan_job.is_some()
            || self.sync.busy()
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_scan(&ctx);
        self.poll_action(&ctx);
        self.poll_plan_job(&ctx);
        self.poll_apply_job(&ctx);
        self.poll_fetch_job(&ctx);
        self.poll_set_check(&ctx);
        self.poll_gh_job(&ctx);
        if self.install.is_some() && !self.mods_loaded && self.scan_job.is_none() {
            self.kick_scan(&ctx);
        }
        if !self.update_checked {
            self.update_checked = true;
            self.kick_update_check(&ctx);
        }
        self.poll_update_check();

        egui::Panel::top("topbar")
            .frame(
                egui::Frame::default()
                    .fill(ctx.global_style().visuals.window_fill)
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show_inside(ui, |ui| self.ui_topbar(ui));

        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(176.0)
            .frame(
                egui::Frame::default()
                    .fill(ctx.global_style().visuals.panel_fill)
                    .inner_margin(egui::Margin::same(10)),
            )
            .show_inside(ui, |ui| self.ui_nav(ui, &ctx));

        egui::Frame::default()
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| match self.tab {
                Tab::Mods => self.ui_mods(ui, &ctx),
                Tab::Sync => self.ui_sync(ui, &ctx),
                Tab::Profiles => self.ui_profiles(ui, &ctx),
                Tab::Publish => self.ui_publish(ui, &ctx),
            });
    }
}

impl App {
    fn ui_topbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("sts2-modsync").heading().color(ACCENT));
            ui.add_space(10.0);
            match &self.install {
                Some(i) => {
                    ui.colored_label(OK, "●");
                    ui.label(
                        egui::RichText::new(format!(
                            "StS2 {}",
                            i.version.as_deref().unwrap_or("?")
                        ))
                        .weak(),
                    );
                }
                None => {
                    ui.colored_label(WARN, "● juego no detectado");
                }
            }
            if self.game_running {
                ui.colored_label(WARN, "· ABIERTO");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let has = self.install.is_some();
                if ui.add_enabled(has, egui::Button::new("▶ Jugar")).clicked() {
                    let r = self.install.as_ref().map(launch::launch);
                    if let Some(r) = r {
                        match r {
                            Ok(()) => self.show_toast("lanzando el juego...", false),
                            Err(e) => self.show_toast(format!("{e:#}"), true),
                        }
                    }
                }
                if ui.button("Elegir carpeta").clicked() {
                    match detect::pick_folder_dialog() {
                        Some(i) => self.accept_install(i),
                        None => self.install_note = "Carpeta invalida.".into(),
                    }
                }
                if ui.button("Re-detectar").clicked() {
                    self.try_detect();
                }
            });
        });

        // Aviso del install (se setea en try_detect/Elegir carpeta): antes se guardaba pero
        // nunca se renderizaba, asi que el usuario elegia mal la carpeta y no veia nada.
        if !self.install_note.is_empty() {
            ui.colored_label(WARN, &self.install_note);
        }

        // Banner de auto-update. No actualizar (self-replace + relaunch) mientras corre
        // CUALQUIER job: hacerlo en medio de un apply corromperia el set.
        if let Some(rel) = self.update_avail.clone() {
            let can = !self.any_job();
            let mut do_update = false;
            ui.horizontal(|ui| {
                ui.colored_label(ACCENT, format!("● Version nueva {} disponible", rel.tag));
                if ui
                    .add_enabled(can, egui::Button::new("Actualizar ahora"))
                    .clicked()
                {
                    do_update = true;
                }
            });
            // Notas del release ANTES de actualizar (que sabe que cambia).
            if !rel.notes.trim().is_empty() {
                ui.collapsing("Ver notas del release", |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(rel.notes.trim()).weak());
                        });
                });
            }
            if do_update {
                let ctx = ui.ctx().clone();
                self.start_update(&ctx, rel);
            }
        }
    }

    fn ui_nav(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(4.0);
        if nav_item(ui, self.tab == Tab::Mods, "Mods") {
            self.tab = Tab::Mods;
        }
        if nav_item(ui, self.tab == Tab::Sync, "Sync") {
            self.tab = Tab::Sync;
        }
        if nav_item(ui, self.tab == Tab::Profiles, "Perfiles") {
            self.tab = Tab::Profiles;
        }
        if nav_item(ui, self.tab == Tab::Publish, "Publicar") {
            self.tab = Tab::Publish;
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            let txt = if self.dark_mode {
                "Tema claro"
            } else {
                "Tema oscuro"
            };
            if ui.button(txt).clicked() {
                self.dark_mode = !self.dark_mode;
                apply_theme(ctx, self.dark_mode);
            }
        });
    }
}
