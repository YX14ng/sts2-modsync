//! GUI del mod manager (eframe/egui, single-exe, backend glow). Pestañas:
//! **Mods** (gestor: lista/detalle/on-off/instalar/desinstalar) · **Sync** (el añadido:
//! cargar un set-manifest, revisar el plan, aplicar) · **Perfiles** (conjuntos de mods
//! habilitados). Es una cascara sobre el core (detect/modlist/manager/profile/sync): NO
//! duplica logica. Todo el trabajo pesado (scan, mover/copiar carpetas, hashing) corre en
//! hilos aparte y se comunica por canales `mpsc`; la UI sondea en `ui()` (eframe 0.34) y
//! pide `ctx.request_repaint()`. NUNCA se bloquea el loop de egui. enable/disable mueven
//! carpetas (NO tocan `setting.save`); el orden lo impone ModListSorter.

use crate::detect::{self, Install};
use crate::manifest::SetManifest;
use crate::modlist::{self, InstalledMod};
use crate::profile::{self, Profile};
use crate::{config, launch, manager, publish, sync, transport, update};
use eframe::egui;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

const WARN: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x6C, 0x00);
const OK: egui::Color32 = egui::Color32::from_rgb(0x3F, 0xB9, 0x50);
const BAD: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x57, 0x57);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x7C, 0x9C, 0xFF);

/// Resultado del worker que baja el set-manifest + su `.minisig` opcional.
type FetchResult = std::result::Result<(String, Option<String>), String>;

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

/// Seccion enmarcada ("card") reusable para darle jerarquia al contenido.
fn card<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
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
fn nav_item(ui: &mut egui::Ui, selected: bool, label: &str) -> bool {
    let w = ui.available_width();
    ui.add_sized([w, 32.0], egui::Button::selectable(selected, label))
        .clicked()
}

pub fn run() -> eframe::Result {
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
    selected: Option<String>,
    confirm_uninstall: Option<String>,

    // Accion en curso (enable/disable/install/uninstall/aplicar perfil): una a la vez.
    action_job: Option<Receiver<Result<String, String>>>,
    busy: String,
    toast: Option<(String, bool)>,

    // Pestaña Sync
    sync: SyncState,

    // Pestaña Perfiles
    profiles: Vec<Profile>,
    profiles_loaded: bool,
    new_profile: String,

    // Pestaña Publicar
    pub_name: String,
    pub_version: String,
    pub_base_url: String,
    pub_profile: Option<String>, // None = mods habilitados actuales
    pub_out_dir: Option<PathBuf>,

    // Auto-update
    update_checked: bool,
    update_check_job: Option<Receiver<Option<update::Release>>>,
    update_avail: Option<update::Release>,

    dark_mode: bool,
}

impl App {
    fn new() -> Self {
        let mut app = App {
            tab: Tab::Mods,
            cfg: config::load(),
            install: None,
            install_note: String::new(),
            game_running: false,
            mods: Vec::new(),
            mods_loaded: false,
            scan_job: None,
            filter: String::new(),
            selected: None,
            confirm_uninstall: None,
            action_job: None,
            busy: String::new(),
            toast: None,
            sync: SyncState::default(),
            profiles: Vec::new(),
            profiles_loaded: false,
            new_profile: String::new(),
            pub_name: String::new(),
            pub_version: String::new(),
            pub_base_url: String::new(),
            pub_profile: None,
            pub_out_dir: None,
            update_checked: false,
            update_check_job: None,
            update_avail: None,
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
                self.toast = Some((e, true));
                self.mods_loaded = true;
                self.scan_job = None;
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => self.scan_job = None,
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
                self.toast = Some(match res {
                    Ok(m) => (m, false),
                    Err(e) => (e, true),
                });
                self.mods_loaded = false; // refrescar lista
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => {
                self.action_job = None;
                self.busy.clear();
                self.mods_loaded = false;
                self.toast = Some((
                    "la operacion no se completo (worker terminado)".into(),
                    true,
                ));
            }
        }
    }

    /// True si hay CUALQUIER trabajo de fondo en curso (scan, accion, plan, fetch, apply).
    /// Usado para no disparar acciones destructivas (update, cargar set) en paralelo.
    fn any_job(&self) -> bool {
        !self.busy.is_empty()
            || self.action_job.is_some()
            || self.scan_job.is_some()
            || self.sync.plan_job.is_some()
            || self.sync.fetch_job.is_some()
            || self.sync.apply_job.is_some()
    }

    fn toggle_mod(&mut self, ctx: &egui::Context, id: &str) {
        let Some(install) = self.install.clone() else {
            return;
        };
        let enabled = self.mods.iter().any(|m| m.id() == id && m.enabled);
        let id = id.to_string();
        let verb = if enabled {
            "deshabilitando"
        } else {
            "habilitando"
        };
        self.run_action(ctx, format!("{verb} {id}..."), move || {
            if enabled {
                manager::disable(&install, &id)?;
                Ok(format!("deshabilitado: {id}"))
            } else {
                manager::enable(&install, &id)?;
                Ok(format!("habilitado: {id}"))
            }
        });
    }
}

// ---- estado de la pestaña Sync (el añadido) --------------------------------

#[derive(Default, PartialEq, Eq)]
enum SyncScreen {
    #[default]
    Review,
    Progress,
}

enum SyncProgress {
    Status(String),
    Bytes { done: u64, total: u64 },
    Done,
    Failed(String),
}

#[derive(Default)]
struct ProgressState {
    status: String,
    done: u64,
    total: u64,
    finished: bool,
    error: Option<String>,
}

#[derive(Default)]
struct SyncState {
    screen: SyncScreen,
    url: String,    // input de URL del set
    source: String, // etiqueta del set cargado (archivo o URL), vacia = nada cargado
    manifest: Option<SetManifest>,
    load_err: Option<String>,
    plan: Option<sync::Plan>,
    plan_job: Option<Receiver<Result<sync::Plan, String>>>,
    consent: bool,
    /// Descarga del set-manifest (+ su `.minisig` opcional) por URL (worker).
    fetch_job: Option<Receiver<FetchResult>>,
    apply_job: Option<Receiver<SyncProgress>>,
    prog: ProgressState,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_scan(&ctx);
        self.poll_action(&ctx);
        self.poll_plan_job(&ctx);
        self.poll_apply_job(&ctx);
        self.poll_fetch_job(&ctx);
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
                        self.toast = Some(match r {
                            Ok(()) => ("lanzando el juego...".into(), false),
                            Err(e) => (format!("{e:#}"), true),
                        });
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

// ---- Pestaña Mods ----------------------------------------------------------

impl App {
    fn ui_mods(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.install.is_none() {
            ui.label("Detecta o elegi la carpeta del juego (arriba) para ver los mods.");
            return;
        }

        let mut pick_dir = false;
        let mut pick_zip = false;
        ui.horizontal(|ui| {
            ui.label("Buscar:");
            ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(180.0));
            if ui.button("Instalar carpeta...").clicked() {
                pick_dir = true;
            }
            if ui.button("Instalar .zip...").clicked() {
                pick_zip = true;
            }
            if ui.button("Re-escanear").clicked() {
                self.mods_loaded = false;
            }
        });
        if pick_dir {
            self.install_picked(ctx, false);
        }
        if pick_zip {
            self.install_picked(ctx, true);
        }

        if !self.busy.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(self.busy.as_str());
            });
        }
        if let Some((msg, err)) = &self.toast {
            ui.colored_label(if *err { BAD } else { OK }, msg);
        }

        if self.scan_job.is_some() || !self.mods_loaded {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Escaneando mods...");
            });
            return;
        }

        // Warnings agregados.
        let missing = modlist::missing_dependencies(&self.mods);
        if !missing.is_empty() {
            let txt: Vec<String> = missing.iter().map(|(m, d)| format!("{m}→{d}")).collect();
            ui.colored_label(BAD, format!("Dependencias faltantes: {}", txt.join(", ")));
        }
        let conflicts = modlist::conflicts(&self.mods);
        if !conflicts.is_empty() {
            ui.colored_label(
                BAD,
                format!("Conflictos (ids duplicados): {}", conflicts.join(", ")),
            );
        }
        ui.label(format!(
            "Orden de carga (multiplayer): {}",
            modlist::load_order(&self.mods).join("  →  ")
        ));
        if !modlist::load_order_enforced(&self.mods) {
            ui.colored_label(
                WARN,
                "ModListSorter deshabilitado: el orden de carga puede divergir entre amigos (room-hash).",
            );
        }
        ui.separator();

        let (n_on, n_off) = self.mods.iter().fold((0, 0), |(on, off), m| {
            if m.enabled {
                (on + 1, off)
            } else {
                (on, off + 1)
            }
        });
        let can_act = self.busy.is_empty() && self.action_job.is_none() && !self.game_running;
        let filter = self.filter.to_ascii_lowercase();

        let mut toggle: Option<String> = None;
        let mut select: Option<String> = None;

        card(
            ui,
            &format!("Mods  ·  {n_on} habilitados  ·  {n_off} deshabilitados"),
            |ui| {
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for i in 0..self.mods.len() {
                            let m = &self.mods[i];
                            if !filter.is_empty() && !mod_matches(m, &filter) {
                                continue;
                            }
                            let id = m.id().to_string();
                            let mut on = m.enabled;
                            let gameplay = m.manifest.affects_gameplay;
                            let name = m.manifest.display_name().to_string();
                            let ver = m.manifest.version.clone().unwrap_or_else(|| "?".into());
                            let size = human_size(m.size_bytes);
                            let is_sel = self.selected.as_deref() == Some(id.as_str());

                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(can_act, egui::Checkbox::new(&mut on, ""))
                                    .changed()
                                {
                                    toggle = Some(id.clone());
                                }
                                let label = format!("{name}  ·  {ver}  ·  {size}");
                                if ui.selectable_label(is_sel, label).clicked() {
                                    select = Some(id.clone());
                                }
                                if gameplay {
                                    ui.colored_label(WARN, "gameplay");
                                }
                            });
                        }
                    });
            },
        );

        if let Some(id) = select {
            self.selected = Some(id);
        }
        if let Some(id) = toggle {
            self.toggle_mod(ctx, &id);
        }

        ui.separator();
        self.ui_mod_details(ui, ctx, can_act);
    }

    fn ui_mod_details(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, can_act: bool) {
        let Some(id) = self.selected.clone() else {
            ui.label(egui::RichText::new("Eleg\u{ed} un mod para ver su detalle.").weak());
            return;
        };
        let Some(m) = self.mods.iter().find(|m| m.id() == id).cloned() else {
            return;
        };

        ui.label(egui::RichText::new(m.manifest.display_name()).strong());
        ui.label(format!(
            "id: {}   ·   v{}   ·   {}   ·   por {}",
            m.id(),
            m.manifest.version.as_deref().unwrap_or("?"),
            human_size(m.size_bytes),
            m.manifest.author.as_deref().unwrap_or("?"),
        ));
        let mut flags = Vec::new();
        if m.manifest.has_dll {
            flags.push("dll");
        }
        if m.manifest.has_pck {
            flags.push("pck");
        }
        if m.manifest.affects_gameplay {
            flags.push("gameplay");
        }
        flags.push(if m.enabled {
            "habilitado"
        } else {
            "deshabilitado"
        });
        ui.label(flags.join(" · "));
        if let Some(d) = &m.manifest.description {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(d).weak());
        }
        if !m.manifest.dependencies.is_empty() {
            let enabled: std::collections::BTreeSet<&str> = self
                .mods
                .iter()
                .filter(|x| x.enabled)
                .map(|x| x.id())
                .collect();
            ui.horizontal_wrapped(|ui| {
                ui.label("Depende de:");
                for dep in &m.manifest.dependencies {
                    if enabled.contains(dep.as_str()) {
                        ui.label(dep);
                    } else {
                        ui.colored_label(BAD, format!("{dep} (falta)"));
                    }
                }
            });
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let toggle_txt = if m.enabled {
                "Deshabilitar"
            } else {
                "Habilitar"
            };
            if ui
                .add_enabled(can_act, egui::Button::new(toggle_txt))
                .clicked()
            {
                self.toggle_mod(ctx, &id);
            }
            if ui.button("Abrir carpeta").clicked() {
                let _ = manager::open_folder(&m.dir);
            }
            if self.confirm_uninstall.as_deref() == Some(id.as_str()) {
                ui.colored_label(BAD, "¿Seguro?");
                if ui
                    .add_enabled(can_act, egui::Button::new("Si, a la papelera"))
                    .clicked()
                {
                    self.confirm_uninstall = None;
                    self.uninstall_mod(ctx, &id);
                }
                if ui.button("No").clicked() {
                    self.confirm_uninstall = None;
                }
            } else if ui
                .add_enabled(can_act, egui::Button::new("Desinstalar"))
                .clicked()
            {
                self.confirm_uninstall = Some(id.clone());
            }
        });
    }

    fn install_picked(&mut self, ctx: &egui::Context, zip: bool) {
        let Some(install) = self.install.clone() else {
            return;
        };
        let picked = if zip {
            rfd::FileDialog::new()
                .add_filter("zip", &["zip"])
                .set_title("Elegi el .zip del mod")
                .pick_file()
        } else {
            rfd::FileDialog::new()
                .set_title("Elegi la carpeta del mod (con su <id>.json)")
                .pick_folder()
        };
        let Some(path) = picked else { return };
        self.run_action(ctx, "instalando mod...".into(), move || {
            let id = if zip {
                manager::install_from_zip(&install, &path, false)?
            } else {
                manager::install_from_dir(&install, &path, false)?
            };
            Ok(format!("instalado: {id}"))
        });
    }

    fn uninstall_mod(&mut self, ctx: &egui::Context, id: &str) {
        let Some(install) = self.install.clone() else {
            return;
        };
        let id = id.to_string();
        self.run_action(ctx, format!("desinstalando {id}..."), move || {
            manager::uninstall(&install, &id)?;
            Ok(format!("desinstalado (papelera): {id}"))
        });
        self.selected = None;
    }
}

fn mod_matches(m: &InstalledMod, filter_lower: &str) -> bool {
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

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

// ---- Pestaña Perfiles ------------------------------------------------------

impl App {
    fn ui_profiles(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.install.is_none() {
            ui.label("Detecta el juego para usar perfiles.");
            return;
        }
        if !self.profiles_loaded {
            self.profiles = profile::list();
            self.profiles_loaded = true;
        }
        ui.label("Un perfil = un conjunto de mods habilitados. Aplicar uno deja exactamente esos.");
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label("Guardar el set actual como:");
            ui.add(egui::TextEdit::singleline(&mut self.new_profile).desired_width(160.0));
            let can_save = !self.new_profile.trim().is_empty();
            if ui
                .add_enabled(can_save, egui::Button::new("Guardar"))
                .clicked()
            {
                let prof = Profile::from_current(self.new_profile.trim(), &self.mods);
                match profile::save(&prof) {
                    Ok(()) => {
                        self.toast = Some((format!("perfil guardado: {}", prof.name), false));
                        self.new_profile.clear();
                        self.profiles_loaded = false;
                    }
                    Err(e) => self.toast = Some((format!("{e:#}"), true)),
                }
            }
        });
        ui.separator();

        let can_act = self.busy.is_empty() && self.action_job.is_none() && !self.game_running;
        let mut apply: Option<Profile> = None;
        let mut delete: Option<String> = None;
        for p in &self.profiles {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&p.name).strong());
                ui.label(egui::RichText::new(format!("({} mods)", p.enabled_ids.len())).weak());
                if ui
                    .add_enabled(can_act, egui::Button::new("Aplicar"))
                    .clicked()
                {
                    apply = Some(p.clone());
                }
                if ui.button("Borrar").clicked() {
                    delete = Some(p.name.clone());
                }
            });
        }
        if self.profiles.is_empty() {
            ui.label(egui::RichText::new("(todavia no hay perfiles guardados)").weak());
        }

        if let Some(p) = apply {
            let install = self.install.clone().unwrap();
            self.run_action(ctx, format!("aplicando perfil {}...", p.name), move || {
                let r = profile::apply(&install, &p)?;
                Ok(format!(
                    "perfil aplicado: +{} -{} (faltan {})",
                    r.enabled.len(),
                    r.disabled.len(),
                    r.not_installed.len()
                ))
            });
        }
        if let Some(name) = delete {
            match profile::delete(&name) {
                Ok(()) => {
                    self.toast = Some((format!("perfil borrado: {name}"), false));
                    self.profiles_loaded = false;
                }
                Err(e) => self.toast = Some((format!("{e:#}"), true)),
            }
        }
    }
}

// ---- Pestaña Publicar (lado modder) ----------------------------------------

impl App {
    /// Ids a publicar segun la fuente elegida (habilitados actuales o un perfil).
    fn publish_ids(&self) -> BTreeSet<String> {
        match &self.pub_profile {
            Some(name) => self
                .profiles
                .iter()
                .find(|p| &p.name == name)
                .map(|p| p.enabled_ids.iter().cloned().collect())
                .unwrap_or_default(),
            None => self
                .mods
                .iter()
                .filter(|m| m.enabled)
                .map(|m| m.id().to_string())
                .collect(),
        }
    }

    fn ui_publish(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.install.is_none() {
            ui.label("Detecta el juego para publicar un set.");
            return;
        }
        if !self.profiles_loaded {
            self.profiles = profile::list();
            self.profiles_loaded = true;
        }
        ui.label(
            "Genera un set-manifest + assets desde tus mods, listo para subir a un GitHub Release.",
        );
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label("Nombre del set:");
            ui.add(egui::TextEdit::singleline(&mut self.pub_name).desired_width(260.0));
        });
        ui.horizontal(|ui| {
            ui.label("Version (= tag del release):");
            ui.add(egui::TextEdit::singleline(&mut self.pub_version).desired_width(160.0));
        });
        ui.horizontal(|ui| {
            ui.label("base_url:");
            ui.add(egui::TextEdit::singleline(&mut self.pub_base_url).desired_width(460.0));
        });
        ui.label(
            egui::RichText::new(
                "base_url = https://github.com/USUARIO/REPO/releases/download/<version>/",
            )
            .weak(),
        );
        ui.add_space(4.0);

        // Fuente: habilitados actuales o un perfil (radios planos, sin closures sobre self).
        ui.label("Mods a publicar:");
        if ui
            .radio(self.pub_profile.is_none(), "los habilitados actuales")
            .clicked()
        {
            self.pub_profile = None;
        }
        let names: Vec<String> = self.profiles.iter().map(|p| p.name.clone()).collect();
        for name in &names {
            let sel = self.pub_profile.as_deref() == Some(name.as_str());
            if ui.radio(sel, format!("perfil: {name}")).clicked() {
                self.pub_profile = Some(name.clone());
            }
        }

        let ids = self.publish_ids();
        for w in publish::warnings(&ids) {
            ui.colored_label(WARN, w);
        }
        ui.label(format!("{} mods seleccionados", ids.len()));

        ui.add_space(6.0);
        let can = self.busy.is_empty()
            && self.action_job.is_none()
            && !self.pub_name.trim().is_empty()
            && !self.pub_version.trim().is_empty()
            && !self.pub_base_url.trim().is_empty()
            && !ids.is_empty();
        if ui
            .add_enabled(can, egui::Button::new("Publicar (subir al release)"))
            .clicked()
        {
            self.start_publish(ctx, ids);
        }
        if !self.busy.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(self.busy.as_str());
            });
        }
        if let Some((msg, err)) = &self.toast {
            ui.colored_label(if *err { BAD } else { OK }, msg);
        }
        if let Some(dir) = self.pub_out_dir.clone()
            && ui.button("Abrir carpeta de salida").clicked()
        {
            let _ = manager::open_folder(&dir);
        }
    }

    fn start_publish(&mut self, ctx: &egui::Context, ids: BTreeSet<String>) {
        let Some(install) = self.install.clone() else {
            return;
        };
        let Some(out_dir) = rfd::FileDialog::new()
            .set_title("Carpeta de salida para el set publicado")
            .pick_folder()
        else {
            return;
        };
        self.pub_out_dir = Some(out_dir.clone());
        let params = publish::PublishParams {
            set_name: self.pub_name.trim().to_string(),
            set_version: self.pub_version.trim().to_string(),
            base_url: self.pub_base_url.trim().to_string(),
            published_at: String::new(),
            baselib_version: None,
        };
        self.run_action(
            ctx,
            "publicando (hasheando + subiendo al release)...".into(),
            move || {
                let mods = modlist::scan(&install)?;
                let prep = publish::prepare(&mods, &ids, &params)?;
                publish::write_out(&prep, &out_dir)?;
                let url = publish::upload(&out_dir, &params.base_url)?;
                Ok(format!(
                    "Publicado: {} assets ({:.1} MB) → {url}",
                    prep.assets.len(),
                    prep.total_bytes() as f64 / 1.0e6,
                ))
            },
        );
    }
}

// ---- Pestaña Sync (el añadido: cargar set-manifest -> revisar -> progreso) --

impl App {
    fn ui_sync(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.install.is_none() {
            ui.label("Detecta el juego para sincronizar un set.");
            return;
        }
        match self.sync.screen {
            SyncScreen::Review => self.ui_sync_review(ui, ctx),
            SyncScreen::Progress => self.ui_sync_progress(ui),
        }
    }

    fn ui_sync_review(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut do_load_url = false;
        let mut do_open_file = false;
        let mut load_saved: Option<String> = None;
        let mut del_saved: Option<String> = None;
        let busy_any = self.any_job();

        card(ui, "Cargar un set", |ui| {
            ui.horizontal(|ui| {
                ui.label("URL:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sync.url)
                        .hint_text("https://.../set-manifest.json")
                        .desired_width(360.0),
                );
                let can = !busy_any && !self.sync.url.trim().is_empty();
                if ui
                    .add_enabled(can, egui::Button::new("Cargar URL"))
                    .clicked()
                {
                    do_load_url = true;
                }
                if ui
                    .add_enabled(!busy_any, egui::Button::new("Abrir archivo..."))
                    .clicked()
                {
                    do_open_file = true;
                }
            });
            if self.sync.fetch_job.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Descargando manifest...");
                });
            }
            if !self.cfg.subscribed_sets.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Sets guardados (1 clic para re-sincronizar):").weak(),
                );
                for s in self.cfg.subscribed_sets.clone() {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!busy_any, egui::Button::new("Cargar"))
                            .clicked()
                        {
                            load_saved = Some(s.clone());
                        }
                        if ui.small_button("borrar").clicked() {
                            del_saved = Some(s.clone());
                        }
                        ui.label(egui::RichText::new(&s).weak());
                    });
                }
            }
        });

        if do_load_url {
            self.load_url(ctx);
        }
        if do_open_file {
            self.open_manifest(ctx);
        }
        if let Some(s) = load_saved {
            self.sync.url = s;
            self.load_url(ctx);
        }
        if let Some(s) = del_saved {
            self.cfg.subscribed_sets.retain(|x| *x != s);
            let _ = config::save(&self.cfg);
        }

        if let Some(e) = &self.sync.load_err {
            ui.colored_label(BAD, format!("Error: {e}"));
        }
        if let Some(m) = &self.sync.manifest {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(format!("{}  v{}", m.set_name, m.set_version)).strong());
            if !self.sync.source.is_empty() {
                ui.label(egui::RichText::new(&self.sync.source).weak());
            }
            if let Some(bl) = &m.baselib_version {
                ui.colored_label(WARN, format!("Requiere BaseLib {bl}."));
            }
        }
        ui.separator();

        if self.sync.plan_job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Calculando plan (hash)...");
            });
            return;
        }
        let (has_plan, is_noop, n_dl, n_orphan) = match &self.sync.plan {
            Some(p) => {
                render_plan(ui, p);
                (true, p.is_noop(), p.to_download.len(), p.orphans.len())
            }
            None => {
                ui.label("Abri un set-manifest para ver el plan.");
                (false, true, 0, 0)
            }
        };
        if !has_plan {
            return;
        }
        ui.separator();
        if is_noop {
            ui.colored_label(OK, "Todo sincronizado: nada que instalar.");
        }
        ui.checkbox(
            &mut self.sync.consent,
            format!(
                "Entiendo: {n_dl} archivos a instalar/actualizar y {n_orphan} huerfanos a borrar."
            ),
        );
        let can = self.sync.consent && !is_noop && !self.game_running;
        if ui
            .add_enabled(can, egui::Button::new("Instalar  →"))
            .clicked()
        {
            self.start_apply(ctx);
        }
        if self.game_running {
            ui.colored_label(WARN, "Cerra el juego para instalar.");
        }
    }

    fn ui_sync_progress(&mut self, ui: &mut egui::Ui) {
        ui.label(self.sync.prog.status.as_str());
        let frac = if self.sync.prog.total > 0 {
            self.sync.prog.done as f32 / self.sync.prog.total as f32
        } else if self.sync.prog.finished && self.sync.prog.error.is_none() {
            1.0
        } else {
            0.0
        };
        ui.add(egui::ProgressBar::new(frac).show_percentage());
        if let Some(e) = &self.sync.prog.error {
            ui.colored_label(BAD, format!("No se completo: {e}"));
            ui.label(
                egui::RichText::new(
                    "Revisa la URL del set (base_url) y tu conexion; los .part quedan para reintentar.",
                )
                .italics()
                .weak(),
            );
        } else if self.sync.prog.finished {
            ui.colored_label(OK, "Instalacion completa.");
        }
        ui.add_space(10.0);
        let running = self.sync.apply_job.is_some();
        if ui
            .add_enabled(!running, egui::Button::new("←  Volver"))
            .clicked()
        {
            self.sync.screen = SyncScreen::Review;
        }
    }

    fn open_manifest(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("set-manifest", &["json"])
            .set_title("Elegi el set-manifest")
            .pick_file()
        else {
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                // Firma opcional: un `<archivo>.minisig` al lado.
                let sig = std::fs::read_to_string(format!("{}.minisig", path.display())).ok();
                self.load_from_text(&text, path.display().to_string(), sig, ctx);
            }
            Err(e) => self.sync.load_err = Some(format!("no se pudo leer: {e}")),
        }
    }

    /// Baja el set-manifest de `self.sync.url` en un worker (no bloquea la UI).
    fn load_url(&mut self, ctx: &egui::Context) {
        let url = self.sync.url.trim().to_string();
        if url.is_empty() {
            return;
        }
        self.sync.load_err = None;
        let (tx, rx) = channel();
        self.sync.fetch_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let res = transport::get_text(&url)
                .map(|manifest| {
                    // La firma es opcional (modo dev no la trae): best-effort.
                    let sig = transport::get_text(&format!("{url}.minisig")).ok();
                    (manifest, sig)
                })
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(res);
            ctx.request_repaint();
        });
    }

    fn poll_fetch_job(&mut self, ctx: &egui::Context) {
        let res = match &self.sync.fetch_job {
            Some(rx) => match rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint();
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    self.sync.fetch_job = None;
                    return;
                }
            },
            None => return,
        };
        self.sync.fetch_job = None;
        match res {
            Ok((text, sig)) => {
                let url = self.sync.url.trim().to_string();
                self.load_from_text(&text, url.clone(), sig, ctx);
                // Guardar la suscripcion si cargo bien (1 clic para re-sincronizar despues).
                if self.sync.load_err.is_none()
                    && !url.is_empty()
                    && !self.cfg.subscribed_sets.contains(&url)
                {
                    self.cfg.subscribed_sets.push(url);
                    let _ = config::save(&self.cfg);
                }
            }
            Err(e) => self.sync.load_err = Some(e),
        }
    }

    /// Verifica firma + parsea el manifest + arranca el plan. `source` = etiqueta;
    /// `signature` = contenido del `.minisig` (None si el set no esta firmado / modo dev).
    fn load_from_text(
        &mut self,
        text: &str,
        source: String,
        signature: Option<String>,
        ctx: &egui::Context,
    ) {
        self.sync.load_err = None;
        self.sync.plan = None;
        self.sync.plan_job = None;
        self.sync.consent = false;
        self.sync.manifest = None;
        self.sync.source = source;
        if let Err(e) = crate::signing::verify_with_embedded(text.as_bytes(), signature.as_deref())
        {
            self.sync.load_err = Some(format!("firma invalida: {e:#}"));
            return;
        }
        match SetManifest::from_json_str(text) {
            Ok(m) => {
                self.sync.manifest = Some(m);
                self.start_plan_job(ctx);
            }
            Err(e) => self.sync.load_err = Some(format!("{e:#}")),
        }
    }

    fn start_plan_job(&mut self, ctx: &egui::Context) {
        let (Some(manifest), Some(install)) = (self.sync.manifest.clone(), self.install.clone())
        else {
            return;
        };
        let (tx, rx) = channel();
        self.sync.plan_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let res = sync::plan(&manifest, &install.mods_dir).map_err(|e| format!("{e:#}"));
            let _ = tx.send(res);
            ctx.request_repaint();
        });
    }

    fn poll_plan_job(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.sync.plan_job else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(plan)) => {
                self.sync.plan = Some(plan);
                self.sync.plan_job = None;
            }
            Ok(Err(e)) => {
                self.sync.load_err = Some(e);
                self.sync.plan_job = None;
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => self.sync.plan_job = None,
        }
    }

    fn start_apply(&mut self, ctx: &egui::Context) {
        let (Some(manifest), Some(install)) = (self.sync.manifest.clone(), self.install.clone())
        else {
            return;
        };
        let (tx, rx) = channel();
        self.sync.apply_job = Some(rx);
        self.sync.prog = ProgressState {
            status: "Preparando...".into(),
            ..Default::default()
        };
        self.sync.screen = SyncScreen::Progress;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                if detect::is_game_running() {
                    anyhow::bail!("El juego esta ABIERTO — cerralo antes de instalar.");
                }
                let _ = tx.send(SyncProgress::Status("Calculando plan...".into()));
                ctx.request_repaint();
                let plan = sync::plan(&manifest, &install.mods_dir)?;
                let total = plan.bytes_to_download;
                let _ = tx.send(SyncProgress::Bytes { done: 0, total });
                ctx.request_repaint();
                let source = transport::GitHubReleases::new();
                sync::apply(&plan, &manifest, &install.mods_dir, &source, &mut |done| {
                    let _ = tx.send(SyncProgress::Bytes { done, total });
                    ctx.request_repaint();
                })?;
                Ok(())
            })();
            let _ = match result {
                Ok(()) => tx.send(SyncProgress::Done),
                Err(e) => tx.send(SyncProgress::Failed(format!("{e:#}"))),
            };
            ctx.request_repaint();
        });
    }

    fn poll_apply_job(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.sync.apply_job else {
            return;
        };
        let mut closed = false;
        loop {
            match rx.try_recv() {
                Ok(SyncProgress::Status(s)) => self.sync.prog.status = s,
                Ok(SyncProgress::Bytes { done, total }) => {
                    self.sync.prog.done = done;
                    self.sync.prog.total = total;
                }
                Ok(SyncProgress::Done) => {
                    self.sync.prog.finished = true;
                    self.sync.prog.status = "Listo".into();
                    self.mods_loaded = false; // el set cambio en disco -> re-escanear la lista
                    self.sync.plan = None; // el plan viejo quedo obsoleto
                }
                Ok(SyncProgress::Failed(e)) => {
                    self.sync.prog.finished = true;
                    self.sync.prog.error = Some(e);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    closed = true;
                    break;
                }
            }
        }
        if closed {
            self.sync.apply_job = None;
        } else {
            ctx.request_repaint();
        }
    }
}

fn render_plan(ui: &mut egui::Ui, plan: &sync::Plan) {
    card(ui, "Plan de sincronizacion", |ui| {
        ui.label(format!(
            "Orden de instalacion: {}",
            plan.install_order.join("  →  ")
        ));
        ui.label(format!(
            "Orden de carga (multiplayer): {}",
            plan.load_order.join("  →  ")
        ));
        if !plan.load_order_enforced {
            ui.colored_label(
            WARN,
            "Falta ModListSorter en el set: los amigos pueden quedar con otro orden (room-hash).",
        );
        }
        ui.label(format!(
            "A descargar: {} archivos  ({:.1} MB)  ·  al dia: {}",
            plan.to_download.len(),
            plan.bytes_to_download as f64 / 1.0e6,
            plan.up_to_date.len()
        ));
        egui::ScrollArea::vertical()
            .id_salt("planlist")
            .max_height(140.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for f in &plan.to_download {
                    ui.label(format!(
                        "  + {}   ({:.1} KB)",
                        f.path,
                        f.size as f64 / 1024.0
                    ));
                }
                if !plan.orphans.is_empty() {
                    ui.colored_label(BAD, format!("Huerfanos a borrar: {}", plan.orphans.len()));
                }
            });
    });
}
