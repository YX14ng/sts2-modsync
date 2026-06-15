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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

const WARN: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x6C, 0x00);
const OK: egui::Color32 = egui::Color32::from_rgb(0x3F, 0xB9, 0x50);
const BAD: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x57, 0x57);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x7C, 0x9C, 0xFF);

/// Resultado del worker que baja el set-manifest + su `.minisig` opcional.
/// Resultado del worker que baja el manifest: (texto del manifest, `.minisig` opcional, URL REAL
/// resuelta del manifest). La URL resuelta puede diferir de lo que tipeo el usuario: una
/// suscripcion por repo (`repo:owner/repo`) se resuelve al manifest del ultimo release.
type FetchResult = std::result::Result<(String, Option<String>, String), String>;

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

/// Bytes -> "X.Y MB" (las descargas de mods son escala MB).
fn human_mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

/// Velocidad legible (B/s, KB/s, MB/s).
fn human_speed(bps: f64) -> String {
    if bps >= 1_048_576.0 {
        format!("{:.1} MB/s", bps / 1_048_576.0)
    } else if bps >= 1024.0 {
        format!("{:.0} KB/s", bps / 1024.0)
    } else {
        format!("{:.0} B/s", bps.max(0.0))
    }
}

/// Segundos restantes -> "Xm Ys" / "Xs" / "—" si no se puede estimar.
fn human_eta(secs: f64) -> String {
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
fn toast_hint(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if m.contains("abierto") || m.contains("juego") {
        "Cerra Slay the Spire 2 y reintenta (lockea sus archivos mientras corre)."
    } else if m.contains("espacio") {
        "Libera espacio en disco y reintenta."
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

/// Notificacion efimera (toast). Los exitos se auto-descartan a los pocos segundos; los
/// errores quedan hasta que el usuario los cierra y muestran un hint accionable.
struct Toast {
    msg: String,
    is_error: bool,
    at: std::time::Instant,
}

/// Eventos del worker de login de GitHub (PAT o device-flow).
enum GhEvent {
    DeviceCode { user_code: String, uri: String }, // mostrar y abrir el navegador
    Connected(String),                             // login del usuario
    Disconnected,
    Failed(String),
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

    /// Worker: baja el manifest de cada set suscripto y, si su version es MAS NUEVA que la
    /// ultima sincronizada (`cfg.set_versions`), lo marca como "version nueva disponible".
    fn check_set_updates(&mut self, ctx: &egui::Context) {
        let sets = self.cfg.subscribed_sets.clone();
        let known = self.cfg.set_versions.clone();
        let (tx, rx) = channel();
        self.set_check_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut updates: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            // Sets que NO se pudieron chequear (rate-limit anonimo de GitHub 60/h, sin conexion,
            // repo sin releases). Se reportan al usuario: "Buscar actualizaciones" no debe quedar mudo.
            let mut failed = 0usize;
            for key in &sets {
                // Suscripcion por repo -> resolver la URL del ultimo release; URL fija -> tal cual.
                let url = match config::as_repo_sub(key) {
                    Some(owner_repo) => match owner_repo.split_once('/') {
                        Some((owner, repo)) => {
                            match transport::resolve_latest_manifest(owner, repo) {
                                Ok(u) => u,
                                Err(_) => {
                                    failed += 1;
                                    continue;
                                }
                            }
                        }
                        None => {
                            failed += 1;
                            continue;
                        }
                    },
                    None => key.clone(),
                };
                match transport::get_text(&url)
                    .ok()
                    .and_then(|t| SetManifest::from_json_str(&t).ok())
                {
                    Some(m) => {
                        if let Some(cur) = known.get(key)
                            && update::is_newer(&m.set_version, cur)
                        {
                            updates.insert(key.clone(), m.set_version.clone());
                        }
                    }
                    None => failed += 1,
                }
            }
            let _ = tx.send((updates, failed));
            ctx.request_repaint();
        });
    }

    fn poll_set_check(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.set_check_job else {
            return;
        };
        match rx.try_recv() {
            Ok((updates, failed)) => {
                self.set_updates = updates;
                self.set_check_job = None;
                if failed > 0 {
                    self.show_toast(
                        format!(
                            "No se pudo chequear {failed} set(s): rate-limit de GitHub (60/h sin login) o sin conexion"
                        ),
                        true,
                    );
                }
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => self.set_check_job = None,
        }
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
    File(String), // archivo que se empieza a bajar
    Bytes { done: u64, total: u64 },
    Done(Option<String>), // nota opcional (p.ej. huerfanos que no se pudieron borrar)
    Failed(String),
}

#[derive(Default)]
struct ProgressState {
    status: String,
    file: String, // archivo que se esta bajando ahora
    done: u64,
    total: u64,
    finished: bool,
    error: Option<String>,
    /// Ultima muestra (bytes, instante) para estimar velocidad/ETA.
    last_sample: Option<(u64, std::time::Instant)>,
    /// Velocidad suavizada (bytes/seg).
    speed_bps: f64,
    /// True si termino por CANCELACION del usuario (se muestra neutro, no como error rojo).
    cancelled: bool,
}

#[derive(Default)]
struct SyncState {
    screen: SyncScreen,
    url: String,                // input de URL del set
    repo_input: String, // input "usuario/repo" para suscribirse por repo (sigue el ultimo release)
    source: String,     // etiqueta del set cargado (archivo o URL), vacia = nada cargado
    loaded_url: Option<String>, // URL DE ORIGEN del set cargado (None si vino de archivo)
    /// Clave estable de la suscripcion del set cargado (entrada de `subscribed_sets`): una URL fija
    /// o `repo:owner/repo`. Bajo ella se registra la version sincronizada. None si vino de archivo.
    sub_key: Option<String>,
    manifest: Option<SetManifest>,
    load_err: Option<String>,
    /// Estado de la verificacion de firma del set cargado (se muestra afirmativo en la UI).
    sig_status: Option<crate::signing::SigStatus>,
    plan: Option<sync::Plan>,
    plan_job: Option<Receiver<Result<sync::Plan, String>>>,
    consent: bool,
    /// Descarga del set-manifest (+ su `.minisig` opcional) por URL (worker).
    fetch_job: Option<Receiver<FetchResult>>,
    apply_job: Option<Receiver<SyncProgress>>,
    /// Flag de cancelacion del apply en curso (lo lee el worker; lo setea el boton Cancelar).
    apply_cancel: Option<Arc<AtomicBool>>,
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
            ui.checkbox(&mut self.sort_enabled_first, "Habilitados primero");
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
        self.render_toast(ui);

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
            // Las que ya estan instaladas (deshabilitadas) se habilitan con un clic.
            let enableable = modlist::enableable_missing_deps(&self.mods);
            if !enableable.is_empty() {
                let can = self.busy.is_empty() && self.action_job.is_none() && !self.game_running;
                let label = format!(
                    "Habilitar {} dependencia(s) ya instalada(s)",
                    enableable.len()
                );
                if ui.add_enabled(can, egui::Button::new(label)).clicked()
                    && let Some(install) = self.install.clone()
                {
                    self.run_action(ctx, "habilitando dependencias...".into(), move || {
                        let mut n = 0;
                        for id in &enableable {
                            manager::enable(&install, id)?;
                            n += 1;
                        }
                        Ok(format!("habilitadas {n} dependencia(s)"))
                    });
                }
            }
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
        onboarding_load_order(ui);
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

        // Orden de display: alfabetico (de `scan`) y, si se pidio, habilitados primero (estable).
        let order: Vec<usize> = {
            let mut idx: Vec<usize> = (0..self.mods.len()).collect();
            if self.sort_enabled_first {
                idx.sort_by_key(|&i| !self.mods[i].enabled);
            }
            idx
        };

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
                        for &i in &order {
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
                        self.show_toast(format!("perfil guardado: {}", prof.name), false);
                        self.new_profile.clear();
                        self.profiles_loaded = false;
                    }
                    Err(e) => self.show_toast(format!("{e:#}"), true),
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
                    self.show_toast(format!("perfil borrado: {name}"), false);
                    self.profiles_loaded = false;
                }
                Err(e) => self.show_toast(format!("{e:#}"), true),
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

    /// Valida UNA vez el token guardado en el llavero (whoami -> gh_user). Best-effort.
    fn gh_check_stored(&mut self, ctx: &egui::Context) {
        if self.gh_user_checked || self.gh_job.is_some() {
            return;
        }
        self.gh_user_checked = true;
        let Some(token) = crate::github::load_token() else {
            return;
        };
        let (tx, rx) = channel();
        self.gh_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let ev = match crate::github::Api::new(token).whoami() {
                Ok(login) => GhEvent::Connected(login),
                Err(_) => GhEvent::Disconnected, // token guardado ya no sirve
            };
            let _ = tx.send(ev);
            ctx.request_repaint();
        });
    }

    /// Guarda el PAT pegado y lo valida (whoami). Si no valida, no se guarda.
    fn gh_connect_pat(&mut self, ctx: &egui::Context) {
        let token = self.gh_pat.trim().to_string();
        if token.is_empty() {
            return;
        }
        self.gh_pat.clear();
        let (tx, rx) = channel();
        self.gh_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let ev = match crate::github::Api::new(token.clone()).whoami() {
                Ok(login) => match crate::github::store_token(&token) {
                    Ok(()) => GhEvent::Connected(login),
                    Err(e) => {
                        GhEvent::Failed(format!("token valido pero no se pudo guardar: {e:#}"))
                    }
                },
                Err(e) => GhEvent::Failed(format!("token invalido o sin permiso: {e:#}")),
            };
            let _ = tx.send(ev);
            ctx.request_repaint();
        });
    }

    /// Arranca el OAuth device-flow: pide el codigo, lo muestra (la UI abre el link) y poll-ea
    /// hasta que el usuario autoriza.
    fn gh_connect_device(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        self.gh_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let dc = match crate::github::device_start() {
                Ok(dc) => dc,
                Err(e) => {
                    let _ = tx.send(GhEvent::Failed(format!("{e:#}")));
                    ctx.request_repaint();
                    return;
                }
            };
            let _ = tx.send(GhEvent::DeviceCode {
                user_code: dc.user_code.clone(),
                uri: dc.verification_uri.clone(),
            });
            ctx.request_repaint();
            let mut interval = dc.interval.max(5);
            // Deadline del lado cliente: corta el poll aunque GitHub nunca mande expired_token.
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_secs(dc.expires_in.clamp(60, 1800));
            let mut errors: u32 = 0;
            let ev = loop {
                std::thread::sleep(std::time::Duration::from_secs(interval));
                if std::time::Instant::now() >= deadline {
                    break GhEvent::Failed("el codigo expiro, reintenta".into());
                }
                match crate::github::device_poll(&dc.device_code) {
                    Ok(crate::github::DevicePoll::Token(t)) => {
                        break match crate::github::Api::new(t.clone()).whoami() {
                            Ok(login) => match crate::github::store_token(&t) {
                                Ok(()) => GhEvent::Connected(login),
                                Err(e) => GhEvent::Failed(format!(
                                    "token valido pero no se guardo: {e:#}"
                                )),
                            },
                            Err(e) => GhEvent::Failed(format!("{e:#}")),
                        };
                    }
                    Ok(crate::github::DevicePoll::Pending) => errors = 0,
                    Ok(crate::github::DevicePoll::SlowDown) => {
                        interval += 5;
                        errors = 0;
                    }
                    Ok(crate::github::DevicePoll::Denied) => {
                        break GhEvent::Failed("autorizacion denegada".into());
                    }
                    Ok(crate::github::DevicePoll::Expired) => {
                        break GhEvent::Failed("el codigo expiro, reintenta".into());
                    }
                    // Error TRANSITORIO (red/parse): reintentar; cortar recien tras varios seguidos.
                    Err(e) => {
                        errors += 1;
                        if errors >= 6 {
                            break GhEvent::Failed(format!("device-flow fallando: {e:#}"));
                        }
                    }
                }
            };
            let _ = tx.send(ev);
            ctx.request_repaint();
        });
    }

    fn gh_disconnect(&mut self) {
        let _ = crate::github::clear_token();
        self.gh_user = None;
        self.gh_device = None;
    }

    fn poll_gh_job(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.gh_job else {
            return;
        };
        match rx.try_recv() {
            Ok(GhEvent::DeviceCode { user_code, uri }) => {
                self.gh_device = Some((user_code, uri));
                ctx.request_repaint();
            }
            Ok(GhEvent::Connected(login)) => {
                self.gh_user = Some(login.clone());
                self.gh_device = None;
                self.gh_job = None;
                self.show_toast(format!("conectado a GitHub como {login}"), false);
            }
            Ok(GhEvent::Disconnected) => {
                self.gh_user = None;
                self.gh_device = None;
                self.gh_job = None;
            }
            Ok(GhEvent::Failed(e)) => {
                self.gh_device = None;
                self.gh_job = None;
                self.show_toast(e, true);
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => self.gh_job = None,
        }
    }

    /// Card "Conectar con GitHub" en la pestaña Publicar.
    fn ui_github_connect(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        card(ui, "GitHub (para publicar sin el `gh` CLI)", |ui| {
            // Conectado = hay token en el llavero (aunque whoami no haya validado el nombre aun,
            // p.ej. por un blip de red al arrancar): NO mostramos "desconectado" por eso.
            if self.gh_user.is_some() || crate::github::is_connected() {
                let label = match &self.gh_user {
                    Some(u) => format!("✓ Conectado como {u}"),
                    None => "✓ Token de GitHub guardado".to_string(),
                };
                ui.horizontal(|ui| {
                    ui.colored_label(OK, label);
                    if ui.button("Desconectar").clicked() {
                        self.gh_disconnect();
                    }
                });
                return;
            }
            if let Some((code, uri)) = self.gh_device.clone() {
                ui.label("Abri este link e ingresa el codigo:");
                ui.hyperlink(uri);
                ui.label(egui::RichText::new(&code).heading().strong());
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("esperando que autorices...");
                });
                return;
            }
            if self.gh_job.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("conectando...");
                });
                return;
            }
            ui.label(
                egui::RichText::new(
                    "Opcional: conecta GitHub para que 'Publicar' suba directo por la API (sin \
                     instalar el gh CLI). Si no, se usa gh como fallback.",
                )
                .weak(),
            );
            ui.horizontal(|ui| {
                ui.label("Token (PAT):");
                ui.add(
                    egui::TextEdit::singleline(&mut self.gh_pat)
                        .password(true)
                        .hint_text("PAT classic con scope public_repo (o crea el repo a mano)")
                        .desired_width(320.0),
                );
                if ui.button("Guardar token").clicked() {
                    self.gh_connect_pat(ctx);
                }
            });
            if crate::github::device_flow_enabled()
                && ui.button("Conectar con GitHub (device-flow)").clicked()
            {
                self.gh_connect_device(ctx);
            }
        });
        ui.add_space(6.0);
    }

    fn ui_publish(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.install.is_none() {
            ui.label("Detecta el juego para publicar un set.");
            return;
        }
        self.gh_check_stored(ctx);
        self.ui_github_connect(ui, ctx);
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
            ui.label("Repositorio:");
            ui.add(
                egui::TextEdit::singleline(&mut self.pub_repo)
                    .hint_text("usuario/repo (se recuerda)")
                    .desired_width(300.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Version (= tag del release):");
            ui.add(egui::TextEdit::singleline(&mut self.pub_version).desired_width(160.0));
        });
        // Mostrar a donde va exactamente: cada publicacion es un RELEASE NUEVO en ese repo.
        if let Some(repo) = crate::github::normalize_repo(&self.pub_repo) {
            let ver = self.pub_version.trim();
            if ver.is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "→ release '<version>' en github.com/{repo}  (actualizar = otro release, NO otro repo)"
                    ))
                    .weak(),
                );
            } else if let Some(tag) = crate::github::valid_tag(ver) {
                ui.label(
                    egui::RichText::new(format!(
                        "→ release '{tag}' en github.com/{repo}  (actualizar = otro release, NO otro repo)"
                    ))
                    .weak(),
                );
            } else {
                ui.colored_label(
                    WARN,
                    format!("Version invalida: '{ver}' (sin espacios ni '/'; ej v1.2.3)"),
                );
            }
        } else if !self.pub_repo.trim().is_empty() {
            ui.colored_label(
                WARN,
                "Repositorio invalido: usa 'usuario/repo' o la URL del repo.",
            );
        }
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
            && crate::github::valid_tag(&self.pub_version).is_some()
            && crate::github::normalize_repo(&self.pub_repo).is_some()
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
        self.render_toast(ui);
        if let Some(dir) = self.pub_out_dir.clone()
            && ui.button("Abrir carpeta de salida").clicked()
        {
            let _ = manager::open_folder(&dir);
        }

        // Seeding P2P: comparti el set por torrent mientras la app este abierta. Tus amigos
        // bajan de vos (mas el fallback HTTP del release). Necesita un set ya publicado
        // (set.torrent + assets/ en la carpeta de salida).
        self.ui_seed_control(ui, ctx);
    }

    fn ui_seed_control(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(8.0);
        ui.separator();
        if let Some(stop) = &self.seed_stop {
            ui.horizontal(|ui| {
                ui.spinner();
                let st = self
                    .seed_status
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_default();
                ui.label(if st.is_empty() {
                    "seedeando...".into()
                } else {
                    st
                });
            });
            if ui.button("Detener seed").clicked() {
                stop.store(true, Ordering::Relaxed);
                self.seed_stop = None;
            }
            return;
        }

        let Some(dir) = self.pub_out_dir.clone() else {
            ui.label(egui::RichText::new("Publica un set para poder seedearlo por P2P.").weak());
            return;
        };
        let ready = dir.join("set.torrent").is_file() && dir.join("assets").is_dir();
        if ui
            .add_enabled(ready, egui::Button::new("Seedear este set (P2P)"))
            .clicked()
        {
            self.start_seed(ctx, dir);
        }
        if !ready {
            ui.label(egui::RichText::new("(falta set.torrent/assets: publica primero)").weak());
        }
    }

    fn start_seed(&mut self, ctx: &egui::Context, out_dir: PathBuf) {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let status = self.seed_status.clone();
        let ctx = ctx.clone();
        *status.lock().unwrap() = "iniciando seed...".into();
        std::thread::spawn(move || {
            let assets = out_dir.join("assets");
            let res = (|| -> anyhow::Result<()> {
                let bytes = std::fs::read(out_dir.join("set.torrent"))?;
                crate::torrent::seed_blocking(
                    &assets,
                    &bytes,
                    &mut |st| {
                        if let Ok(mut s) = status.lock() {
                            *s = format!(
                                "seedeando ({}) · subido {:.1} MB",
                                st.state,
                                st.uploaded_bytes as f64 / 1_048_576.0
                            );
                        }
                        ctx.request_repaint();
                    },
                    &|| stop_thread.load(Ordering::Relaxed),
                )
            })();
            if let Ok(mut s) = status.lock() {
                *s = match res {
                    Ok(()) => "seed detenido.".into(),
                    Err(e) => format!("seed cortado: {e:#}"),
                };
            }
            ctx.request_repaint();
        });
        self.seed_stop = Some(stop);
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
        let Some(repo) = crate::github::normalize_repo(&self.pub_repo) else {
            self.show_toast("repositorio invalido (usa usuario/repo)", true);
            return;
        };
        let Some(version) = crate::github::valid_tag(&self.pub_version) else {
            self.show_toast(
                "version/tag invalido (sin espacios ni '/'; ej v1.2.3)",
                true,
            );
            return;
        };
        let set_name = self.pub_name.trim().to_string();
        // RECORDAR el repo + nombre del set: la proxima "actualizacion" reusa el mismo repo
        // (otro release, NO otro repo). Se guarda aunque la subida despues falle.
        self.cfg.publish_repo = Some(repo.clone());
        self.cfg.publish_set_name = Some(set_name.clone());
        let _ = config::save(&self.cfg);
        let params = publish::PublishParams {
            base_url: crate::github::release_base_url(&repo, &version),
            set_name,
            set_version: version,
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
        let mut do_subscribe_repo = false;
        let mut load_saved: Option<String> = None;
        let mut del_saved: Option<String> = None;
        let mut check_updates = false;
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
            // Suscribirse por REPO: sigue el ULTIMO release (no hay que re-pegar la URL al actualizar).
            ui.horizontal(|ui| {
                ui.label("o Repositorio:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sync.repo_input)
                        .hint_text("usuario/repo (sigue el ultimo release)")
                        .desired_width(300.0),
                );
                let can =
                    !busy_any && crate::github::normalize_repo(&self.sync.repo_input).is_some();
                if ui
                    .add_enabled(can, egui::Button::new("Suscribirse"))
                    .clicked()
                {
                    do_subscribe_repo = true;
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
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Sets guardados (1 clic para re-sincronizar):").weak(),
                    );
                    if ui
                        .add_enabled(
                            self.set_check_job.is_none(),
                            egui::Button::new("Buscar actualizaciones"),
                        )
                        .clicked()
                    {
                        check_updates = true;
                    }
                    if self.set_check_job.is_some() {
                        ui.spinner();
                    }
                });
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
                        // Nombre legible (URL cruda en el hover).
                        ui.label(config::set_label(&s)).on_hover_text(&s);
                        if let Some(v) = self.cfg.set_versions.get(&s) {
                            ui.label(egui::RichText::new(format!("v{v}")).weak());
                        }
                        if let Some(newv) = self.set_updates.get(&s) {
                            ui.colored_label(ACCENT, format!("● nueva v{newv}"));
                        }
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
        if do_subscribe_repo {
            self.subscribe_repo(ctx);
        }
        if check_updates {
            self.check_set_updates(ctx);
        }
        if let Some(s) = load_saved {
            // Re-sincronizar un set guardado (URL fija o `repo:owner/repo`): resuelve y baja.
            self.load_source(s, ctx);
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
            // Verificacion de firma VISIBLE y afirmativa (no solo en caso de error).
            match self.sync.sig_status {
                Some(crate::signing::SigStatus::Verified) => {
                    ui.colored_label(OK, "✓ Firma verificada — set autentico del publicador.");
                }
                Some(crate::signing::SigStatus::Unsigned) => {
                    ui.colored_label(
                        WARN,
                        "● Sin firma: confias en que esta URL es del publicador (HTTPS). Los \
                         archivos igual se verifican por hash.",
                    );
                }
                Some(crate::signing::SigStatus::DevUnverified) => {
                    ui.colored_label(
                        WARN,
                        "⚠ Firma NO verificada (modo dev): no confies en este set.",
                    );
                }
                None => {}
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
        let (done, total, speed, finished) = (
            self.sync.prog.done,
            self.sync.prog.total,
            self.sync.prog.speed_bps,
            self.sync.prog.finished,
        );
        let frac = if total > 0 {
            done as f32 / total as f32
        } else if finished && self.sync.prog.error.is_none() {
            1.0
        } else {
            0.0
        };
        ui.add(egui::ProgressBar::new(frac).show_percentage());

        let running = self.sync.apply_job.is_some();
        // Detalle (solo mientras corre): archivo actual + bajado/total + velocidad + ETA.
        if running && !finished {
            if !self.sync.prog.file.is_empty() {
                ui.label(egui::RichText::new(format!("Bajando: {}", self.sync.prog.file)).weak());
            }
            let remaining = total.saturating_sub(done);
            let eta = if speed > 0.0 {
                remaining as f64 / speed
            } else {
                f64::INFINITY
            };
            ui.label(
                egui::RichText::new(format!(
                    "{} / {}   ·   {}   ·   ETA {}",
                    human_mb(done),
                    human_mb(total),
                    human_speed(speed),
                    human_eta(eta),
                ))
                .weak(),
            );
        }

        if self.sync.prog.cancelled {
            ui.colored_label(
                WARN,
                "Cancelado. No se instalo nada; los .part quedan para reanudar.",
            );
        } else if let Some(e) = self.sync.prog.error.clone() {
            ui.colored_label(BAD, format!("No se completo: {e}"));
            ui.label(
                egui::RichText::new(
                    "Revisa la URL del set (base_url) y tu conexion; los .part quedan para reintentar.",
                )
                .italics()
                .weak(),
            );
        } else if finished {
            ui.colored_label(OK, "Instalacion completa.");
        }
        ui.add_space(10.0);

        // Mientras corre: Cancelar. Cuando termino: Volver.
        if running && !finished {
            if ui.add(egui::Button::new("Cancelar")).clicked() {
                if let Some(c) = &self.sync.apply_cancel {
                    c.store(true, Ordering::Relaxed);
                }
                self.sync.prog.status = "Cancelando...".into();
            }
        } else if ui
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
        self.load_source(url, ctx);
    }

    /// Suscribirse a un REPO (`usuario/repo`): sigue el ULTIMO release. Guarda la suscripcion como
    /// `repo:owner/repo` y la carga (resolviendo el ultimo release en el worker).
    fn subscribe_repo(&mut self, ctx: &egui::Context) {
        let Some(repo) = crate::github::normalize_repo(&self.sync.repo_input) else {
            self.sync.load_err = Some("repositorio invalido (usa usuario/repo)".into());
            return;
        };
        let key = config::repo_sub(&repo);
        if !self.cfg.subscribed_sets.contains(&key) {
            self.cfg.subscribed_sets.push(key.clone());
            let _ = config::save(&self.cfg);
        }
        self.sync.repo_input.clear();
        self.load_source(key, ctx);
    }

    /// Arranca la descarga del manifest de una "fuente": una URL https directa o una suscripcion
    /// por repo (`repo:owner/repo`, que el worker resuelve al manifest del ultimo release).
    /// `sub_key` es ademas la clave estable bajo la que se registra la version sincronizada.
    fn load_source(&mut self, sub_key: String, ctx: &egui::Context) {
        let sub_key = sub_key.trim().to_string();
        if sub_key.is_empty() {
            return;
        }
        self.sync.load_err = None;
        self.sync.sub_key = Some(sub_key.clone());
        // Limpiar el set mostrado ANTES de lanzar el fetch: si este falla (404, repo sin releases),
        // no debe quedar instalable el manifest/plan ANTERIOR apareado con el sub_key NUEVO — eso
        // registraria la version del set viejo contra la clave equivocada al instalar.
        self.sync.manifest = None;
        self.sync.plan = None;
        self.sync.plan_job = None;
        self.sync.consent = false;
        self.sync.sig_status = None;
        let (tx, rx) = channel();
        self.sync.fetch_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let res = (|| -> std::result::Result<_, String> {
                // Una suscripcion por repo se resuelve al manifest del ultimo release; una URL va tal cual.
                let url = match config::as_repo_sub(&sub_key) {
                    Some(owner_repo) => {
                        let (owner, repo) = owner_repo
                            .split_once('/')
                            .ok_or_else(|| format!("repo invalido: {owner_repo:?}"))?;
                        transport::resolve_latest_manifest(owner, repo)
                            .map_err(|e| format!("{e:#}"))?
                    }
                    None => sub_key.clone(),
                };
                let manifest = transport::get_text(&url).map_err(|e| format!("{e:#}"))?;
                // La firma es opcional (modo dev no la trae): best-effort.
                let sig = transport::get_text(&format!("{url}.minisig")).ok();
                Ok((manifest, sig, url))
            })();
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
            Ok((text, sig, resolved_url)) => {
                self.load_from_text(&text, resolved_url, sig, ctx);
                // Guardar la suscripcion (URL fija o `repo:owner/repo`) si cargo bien: 1 clic para
                // re-sincronizar despues. Se guarda la CLAVE estable, no la URL resuelta del repo.
                if self.sync.load_err.is_none()
                    && let Some(key) = self.sync.sub_key.clone()
                    && !key.is_empty()
                    && !self.cfg.subscribed_sets.contains(&key)
                {
                    self.cfg.subscribed_sets.push(key);
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
        self.sync.sig_status = None;
        // Recordar la URL de ORIGEN solo si vino por URL (no por archivo).
        self.sync.loaded_url = source.starts_with("http").then(|| source.clone());
        // Un set cargado de ARCHIVO no pertenece a ninguna suscripcion: no registrar su version
        // contra una `sub_key` vieja que quedo de una carga por URL/repo anterior.
        if self.sync.loaded_url.is_none() {
            self.sync.sub_key = None;
        }
        self.sync.source = source;
        // Firma OPCIONAL para sets: si trae firma se valida (firma mala -> Err), si no, Unsigned.
        match crate::signing::verify_optional(text.as_bytes(), signature.as_deref()) {
            Ok(status) => self.sync.sig_status = Some(status),
            Err(e) => {
                self.sync.load_err = Some(format!("firma invalida: {e:#}"));
                return;
            }
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
                // Cold-start del indicador "version nueva": si te suscribiste y ya estas AL DIA
                // (plan noop: nada para bajar ni huerfanos) sin baseline previa, registrala. Asi un
                // amigo que recibio los archivos por otra via (Drive) y despues se suscribe ve el
                // proximo release como "nueva vX" sin tener que hacer un sync redundante primero.
                if plan.is_noop()
                    && let (Some(key), Some(version)) = (
                        self.sync.sub_key.clone(),
                        self.sync.manifest.as_ref().map(|m| m.set_version.clone()),
                    )
                    && self.cfg.subscribed_sets.contains(&key)
                    && !self.cfg.set_versions.contains_key(&key)
                {
                    self.cfg.set_versions.insert(key, version);
                    let _ = config::save(&self.cfg);
                }
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
        let cancel = Arc::new(AtomicBool::new(false));
        self.sync.apply_cancel = Some(cancel.clone());
        self.sync.prog = ProgressState {
            status: "Preparando...".into(),
            ..Default::default()
        };
        self.sync.screen = SyncScreen::Progress;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut last_paint = std::time::Instant::now();
            let result = (|| -> anyhow::Result<Option<String>> {
                if detect::is_game_running() {
                    anyhow::bail!("El juego esta ABIERTO — cerralo antes de instalar.");
                }
                let _ = tx.send(SyncProgress::Status("Calculando plan...".into()));
                ctx.request_repaint();
                let plan = sync::plan(&manifest, &install.mods_dir)?;
                let total = plan.bytes_to_download;
                let _ = tx.send(SyncProgress::Bytes { done: 0, total });
                ctx.request_repaint();
                // Con feature p2p y magnet en el manifest: bajar via torrent (fallback HTTP).
                // Sin eso: solo HTTP, como siempre.
                let source: Box<dyn transport::ModSource> = {
                    #[cfg(feature = "p2p")]
                    {
                        let hy = crate::torrent::HybridSource::new(&manifest);
                        if hy.has_p2p() {
                            let _ = tx.send(SyncProgress::Status(
                                "Bajando via P2P (torrent), fallback HTTP...".into(),
                            ));
                            ctx.request_repaint();
                        }
                        Box::new(hy)
                    }
                    #[cfg(not(feature = "p2p"))]
                    {
                        Box::new(transport::GitHubReleases::new())
                    }
                };
                let report = sync::apply(
                    &plan,
                    &manifest,
                    &install.mods_dir,
                    source.as_ref(),
                    &mut |done| {
                        let _ = tx.send(SyncProgress::Bytes { done, total });
                        // throttle: a lo sumo ~10 repaints/seg (no uno por cada chunk de 64 KB).
                        if last_paint.elapsed() >= std::time::Duration::from_millis(100) {
                            ctx.request_repaint();
                            last_paint = std::time::Instant::now();
                        }
                    },
                    &mut |path| {
                        let _ = tx.send(SyncProgress::File(path.to_string()));
                        ctx.request_repaint();
                    },
                    &cancel,
                )?;
                // No tragar errores: si quedaron huerfanos sin borrar, avisarlo en la pantalla final.
                let note = (!report.orphans_failed.is_empty()).then(|| {
                    format!(
                        "Listo, pero {} huerfano(s) no se pudieron borrar (revisalos a mano).",
                        report.orphans_failed.len()
                    )
                });
                Ok(note)
            })();
            let _ = match result {
                Ok(note) => tx.send(SyncProgress::Done(note)),
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
                Ok(SyncProgress::File(f)) => self.sync.prog.file = f,
                Ok(SyncProgress::Bytes { done, total }) => {
                    self.sync.prog.done = done;
                    self.sync.prog.total = total;
                }
                Ok(SyncProgress::Done(note)) => {
                    self.sync.prog.finished = true;
                    self.sync.prog.done = self.sync.prog.total; // barra al 100%
                    self.sync.prog.file.clear();
                    self.sync.prog.status = note.unwrap_or_else(|| "Listo".into());
                    self.sync.apply_cancel = None;
                    self.mods_loaded = false; // el set cambio en disco -> re-escanear la lista
                    self.sync.plan = None; // el plan viejo quedo obsoleto
                    // Registrar la version sincronizada (alimenta el indicador "version nueva"),
                    // keyed por la CLAVE de la suscripcion (sub_key): una URL fija o `repo:owner/repo`.
                    // Asi una suscripcion por repo conserva su version a traves de releases (la URL
                    // resuelta cambia cada release, la clave no).
                    if let (Some(version), Some(key)) = (
                        self.sync.manifest.as_ref().map(|m| m.set_version.clone()),
                        self.sync.sub_key.clone(),
                    ) && self.cfg.subscribed_sets.contains(&key)
                    {
                        self.cfg.set_versions.insert(key.clone(), version);
                        self.set_updates.remove(&key);
                        let _ = config::save(&self.cfg);
                    }
                }
                Ok(SyncProgress::Failed(e)) => {
                    self.sync.prog.finished = true;
                    self.sync.prog.file.clear();
                    self.sync.apply_cancel = None;
                    // Distinguir una cancelacion del usuario de un fallo real (no es rojo).
                    if e.contains("cancelad") {
                        self.sync.prog.cancelled = true;
                        self.sync.prog.status = "Cancelado".into();
                    } else {
                        self.sync.prog.error = Some(e);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    closed = true;
                    break;
                }
            }
        }
        // Estimar velocidad/ETA del progreso (muestreo >=250 ms, EMA suave).
        if !self.sync.prog.finished {
            let now = std::time::Instant::now();
            match self.sync.prog.last_sample {
                Some((b, t)) => {
                    let dt = now.duration_since(t).as_secs_f64();
                    if dt >= 0.25 {
                        let inst = self.sync.prog.done.saturating_sub(b) as f64 / dt;
                        self.sync.prog.speed_bps = if self.sync.prog.speed_bps <= 0.0 {
                            inst
                        } else {
                            0.7 * self.sync.prog.speed_bps + 0.3 * inst
                        };
                        self.sync.prog.last_sample = Some((self.sync.prog.done, now));
                    }
                }
                None => self.sync.prog.last_sample = Some((self.sync.prog.done, now)),
            }
        }
        if closed {
            self.sync.apply_job = None;
        } else {
            // Heartbeat throttled (~7/seg): mantiene vivo el loop y la velocidad/ETA al dia.
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }
}

/// Explicacion (colapsable) de BaseLib / ModListSorter / orden de carga para no-tecnicos.
/// Onboarding: aparece donde se muestra el orden de carga (multiplayer).
fn onboarding_load_order(ui: &mut egui::Ui) {
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
        onboarding_load_order(ui);
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
