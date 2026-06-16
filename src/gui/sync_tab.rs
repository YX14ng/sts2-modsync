//! Pestaña Sync (el añadido): cargar un set-manifest (URL/archivo/suscripcion por repo),
//! revisar el plan (dry-run con hash) y aplicarlo (descarga transaccional con progreso).
//! Tambien el chequeo manual de "version nueva" de los sets suscritos.

use super::App;
use super::job::Job;
use super::widgets::{card, human_eta, human_mb, human_speed};
use super::{ACCENT, BAD, OK, WARN};
use crate::manifest::SetManifest;
use crate::{config, detect, sync, transport, update};
use eframe::egui;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Resultado del worker que baja el set-manifest + su `.minisig` opcional.
/// Resultado del worker que baja el manifest: (texto del manifest, `.minisig` opcional, URL REAL
/// resuelta del manifest). La URL resuelta puede diferir de lo que tipeo el usuario: una
/// suscripcion por repo (`repo:owner/repo`) se resuelve al manifest del ultimo release.
pub(super) type FetchResult = std::result::Result<(String, Option<String>, String), String>;

#[derive(Default, PartialEq, Eq)]
pub(super) enum SyncScreen {
    #[default]
    Review,
    Progress,
}

pub(super) enum SyncProgress {
    Status(String),
    File(String), // archivo que se empieza a bajar
    Bytes { done: u64, total: u64 },
    Done(Option<String>), // nota opcional (p.ej. huerfanos que no se pudieron borrar)
    Failed(String),
}

#[derive(Default)]
pub(super) struct ProgressState {
    pub(super) status: String,
    pub(super) file: String, // archivo que se esta bajando ahora
    pub(super) done: u64,
    pub(super) total: u64,
    pub(super) finished: bool,
    pub(super) error: Option<String>,
    /// Ultima muestra (bytes, instante) para estimar velocidad/ETA.
    pub(super) last_sample: Option<(u64, std::time::Instant)>,
    /// Velocidad suavizada (bytes/seg).
    pub(super) speed_bps: f64,
    /// True si termino por CANCELACION del usuario (se muestra neutro, no como error rojo).
    pub(super) cancelled: bool,
}

#[derive(Default)]
pub(super) struct SyncState {
    pub(super) screen: SyncScreen,
    pub(super) url: String, // input UNICO: URL del set-manifest, o `usuario/repo` (sigue el ultimo release)
    pub(super) source: String, // etiqueta del set cargado (archivo o URL), vacia = nada cargado
    pub(super) loaded_url: Option<String>, // URL DE ORIGEN del set cargado (None si vino de archivo)
    /// Clave estable de la suscripcion del set cargado (entrada de `subscribed_sets`): una URL fija
    /// o `repo:owner/repo`. Bajo ella se registra la version sincronizada. None si vino de archivo.
    pub(super) sub_key: Option<String>,
    pub(super) manifest: Option<SetManifest>,
    pub(super) load_err: Option<String>,
    /// Estado de la verificacion de firma del set cargado (se muestra afirmativo en la UI).
    pub(super) sig_status: Option<crate::signing::SigStatus>,
    pub(super) plan: Option<sync::Plan>,
    pub(super) plan_job: Job<Result<sync::Plan, String>>,
    pub(super) consent: bool,
    /// Descarga del set-manifest (+ su `.minisig` opcional) por URL (worker).
    pub(super) fetch_job: Job<FetchResult>,
    pub(super) apply_job: Job<SyncProgress>, // STREAMING (varios SyncProgress hasta Done/Failed)
    /// Flag de cancelacion del apply en curso (lo lee el worker; lo setea el boton Cancelar).
    pub(super) apply_cancel: Option<Arc<AtomicBool>>,
    pub(super) prog: ProgressState,
}

impl SyncState {
    /// True si hay un job de sync en curso (plan/fetch/apply). Lo consulta `App::any_job`.
    pub(super) fn busy(&self) -> bool {
        self.plan_job.busy() || self.fetch_job.busy() || self.apply_job.busy()
    }
}

impl App {
    /// Worker: baja el manifest de cada set suscripto y, si su version es MAS NUEVA que la
    /// ultima sincronizada (`cfg.set_versions`), lo marca como "version nueva disponible".
    pub(super) fn check_set_updates(&mut self, ctx: &egui::Context) {
        let sets = self.cfg.subscribed_sets.clone();
        let known = self.cfg.set_versions.clone();
        self.set_check_job.spawn(ctx, move || {
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
            (updates, failed)
        });
    }

    pub(super) fn poll_set_check(&mut self, ctx: &egui::Context) {
        if let Some((updates, failed)) = self.set_check_job.poll(ctx) {
            self.set_check_job.clear();
            self.set_updates = updates;
            if failed > 0 {
                self.show_toast(
                    format!(
                        "No se pudo chequear {failed} set(s): rate-limit de GitHub (60/h sin login) o sin conexion"
                    ),
                    true,
                );
            }
        }
    }

    pub(super) fn ui_sync(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
        let mut do_load = false;
        let mut do_open_file = false;
        let mut load_saved: Option<String> = None;
        let mut del_saved: Option<String> = None;
        let mut check_updates = false;
        let busy_any = self.any_job();

        card(ui, "Cargar un set", |ui| {
            // On-ramp para el amigo que recibe un set: que pegar y donde (el unico tab que antes no
            // lo explicaba). Un solo campo: detecta solo si es un LINK o un `usuario/repo`.
            ui.label(
                egui::RichText::new(
                    "Tu amigo te paso un LINK (https://...) o un usuario/repo de GitHub: pegalo aca y \
                     dale Cargar. Si te paso un archivo set-manifest.json suelto, usa \"Abrir archivo...\".",
                )
                .weak(),
            );
            // `horizontal_wrapped` + anchos moderados: en la ventana minima (700px) la fila no se
            // sale; si no entra, los botones bajan a la linea siguiente en vez de cortarse.
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.sync.url)
                        .hint_text("https://.../set-manifest.json   o   usuario/repo")
                        .desired_width(300.0),
                );
                let can = !busy_any && !self.sync.url.trim().is_empty();
                if ui.add_enabled(can, egui::Button::new("Cargar")).clicked() {
                    do_load = true;
                }
                if ui
                    .add_enabled(!busy_any, egui::Button::new("Abrir archivo..."))
                    .clicked()
                {
                    do_open_file = true;
                }
            });
            if self.sync.fetch_job.busy() {
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
                            !self.set_check_job.busy(),
                            egui::Button::new("Buscar actualizaciones"),
                        )
                        .clicked()
                    {
                        check_updates = true;
                    }
                    if self.set_check_job.busy() {
                        ui.spinner();
                    }
                });
                for s in self.cfg.subscribed_sets.clone() {
                    // `horizontal_wrapped`: una clave/nombre largo hace wrap a la linea siguiente en
                    // vez de empujar la version/el aviso "nueva" fuera de la ventana.
                    ui.horizontal_wrapped(|ui| {
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

        if do_load {
            // Deteccion (igual criterio que la CLI `cmd_sync`): un `usuario/repo` (NO-http que
            // `normalize_repo` acepta) => suscribirse y seguir el ultimo release; cualquier otra cosa
            // (una URL https) => cargar directo. Un link de github.com/<repo> "pagina" no se baja como
            // manifest, pero el hint pide el `usuario/repo` o el link al `set-manifest.json`.
            let input = self.sync.url.trim().to_string();
            match (!input.starts_with("http"))
                .then(|| crate::github::normalize_repo(&input))
                .flatten()
            {
                Some(repo) => self.subscribe_repo(ctx, &repo),
                None => self.load_url(ctx),
            }
        }
        if do_open_file {
            self.open_manifest(ctx);
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
            // Limpiar tambien el baseline de version (persistido) y el flag en memoria de esa
            // suscripcion: si no, re-suscribirse despues resucita un "version nueva" viejo.
            self.cfg.set_versions.remove(&s);
            self.set_updates.remove(&s);
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
            // Avisos de COMPATIBILIDAD: comparar los pines del set (BaseLib / version de StS2) contra
            // lo instalado. Antes solo mostraba "Requiere BaseLib X" pasivo; ahora avisa FUERTE si
            // difiere (skew = crash o desincronizacion del lobby).
            let local_bl = self
                .mods
                .iter()
                .find(|x| x.id() == crate::manifest::BASELIB_ID)
                .and_then(|x| x.manifest.version.as_deref());
            let local_game = self.install.as_ref().and_then(|i| i.version.as_deref());
            for w in m.compatibility_warnings(local_bl, local_game) {
                ui.colored_label(BAD, format!("⚠ {w}"));
            }
            // "Que cambia" a nivel de MOD (no solo bytes): el amigo ve nuevos / actualizados / al dia
            // antes de bajar nada, en vez de una pila de archivos sin contexto.
            let diff = crate::modlist::diff_against_set(&self.mods, m);
            if !diff.is_noop() || diff.up_to_date > 0 {
                let mut parts = Vec::new();
                if !diff.new.is_empty() {
                    parts.push(format!("nuevos {}", diff.new.len()));
                }
                if !diff.updated.is_empty() {
                    parts.push(format!("actualizados {}", diff.updated.len()));
                }
                if diff.up_to_date > 0 {
                    parts.push(format!("ya al dia {}", diff.up_to_date));
                }
                ui.label(
                    egui::RichText::new(format!("Cambios del set: {}", parts.join(" · "))).strong(),
                );
                if !diff.new.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("Nuevos: {}", diff.new.join(", "))).weak(),
                    );
                }
                for (id, old, new) in &diff.updated {
                    ui.label(egui::RichText::new(format!("{id}: v{old} → v{new}")).weak());
                }
            }
        }
        ui.separator();

        if self.sync.plan_job.busy() {
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
            // Clamp a [0,1]: si un delta cae al full, se transfiere mas que lo planeado (el total
            // contaba el patch) y `done` puede superar `total`; la barra no debe pasar el 100%.
            (done as f32 / total as f32).clamp(0.0, 1.0)
        } else if finished && self.sync.prog.error.is_none() {
            1.0
        } else {
            0.0
        };
        ui.add(egui::ProgressBar::new(frac).show_percentage());

        let running = self.sync.apply_job.busy();
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
            ui.label(
                egui::RichText::new(
                    "Para confirmar que entras al MISMO lobby que tus amigos: en la pestaña Mods mira \
                     la \"Huella de orden de carga\" y comparala con ellos (misma huella = mismo lobby).",
                )
                .weak(),
            );
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

    /// Suscribirse a un REPO (`usuario/repo` ya normalizado): sigue el ULTIMO release. Guarda la
    /// suscripcion como `repo:owner/repo` y la carga (resolviendo el ultimo release en el worker).
    fn subscribe_repo(&mut self, ctx: &egui::Context, repo: &str) {
        let key = config::repo_sub(repo);
        if !self.cfg.subscribed_sets.contains(&key) {
            self.cfg.subscribed_sets.push(key.clone());
            let _ = config::save(&self.cfg);
        }
        self.sync.url.clear();
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
        self.sync.plan_job.clear();
        self.sync.consent = false;
        self.sync.sig_status = None;
        self.sync.fetch_job.spawn(ctx, move || {
            (|| -> std::result::Result<_, String> {
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
            })()
        });
    }

    pub(super) fn poll_fetch_job(&mut self, ctx: &egui::Context) {
        let Some(res) = self.sync.fetch_job.poll(ctx) else {
            return;
        };
        self.sync.fetch_job.clear();
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
        self.sync.plan_job.clear();
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
        self.sync.plan_job.spawn(ctx, move || {
            sync::plan(&manifest, &install.mods_dir).map_err(|e| format!("{e:#}"))
        });
    }

    pub(super) fn poll_plan_job(&mut self, ctx: &egui::Context) {
        let Some(res) = self.sync.plan_job.poll(ctx) else {
            return;
        };
        self.sync.plan_job.clear();
        match res {
            Ok(plan) => {
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
            }
            Err(e) => self.sync.load_err = Some(e),
        }
    }

    fn start_apply(&mut self, ctx: &egui::Context) {
        let (Some(manifest), Some(install)) = (self.sync.manifest.clone(), self.install.clone())
        else {
            return;
        };
        let tx = self.sync.apply_job.channel(); // STREAMING: el worker manda varios SyncProgress
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
                // Limpiar carpetas DUPLICADAS de los mods del set (otra copia con otro nombre, o en
                // mods_disabled/) -> a la papelera. Evita dos copias del mismo mod cargando a la vez
                // (room-hash distinto en multiplayer). Best-effort.
                let dups = sync::clean_duplicate_folders(&manifest, &install);

                // No tragar nada: avisar en la pantalla final lo que se limpio y lo que no se pudo.
                let mut notes: Vec<String> = Vec::new();
                if !dups.is_empty() {
                    notes.push(format!(
                        "Se mandaron {} carpeta(s) duplicada(s) a la papelera.",
                        dups.len()
                    ));
                }
                if !report.orphans_failed.is_empty() {
                    notes.push(format!(
                        "{} huerfano(s) no se pudieron borrar (revisalos a mano).",
                        report.orphans_failed.len()
                    ));
                }
                let note = (!notes.is_empty()).then(|| notes.join(" "));
                Ok(note)
            })();
            let _ = match result {
                Ok(note) => tx.send(SyncProgress::Done(note)),
                Err(e) => tx.send(SyncProgress::Failed(format!("{e:#}"))),
            };
            ctx.request_repaint();
        });
    }

    pub(super) fn poll_apply_job(&mut self, ctx: &egui::Context) {
        if !self.sync.apply_job.busy() {
            return;
        }
        // Drenar todos los mensajes disponibles este frame (sin repaint por mensaje: el heartbeat de
        // abajo, throttled, mantiene vivo el loop). `next` limpia el slot solo si el worker murio.
        while let Some(msg) = self.sync.apply_job.next() {
            match msg {
                SyncProgress::Status(s) => self.sync.prog.status = s,
                SyncProgress::File(f) => self.sync.prog.file = f,
                SyncProgress::Bytes { done, total } => {
                    self.sync.prog.done = done;
                    self.sync.prog.total = total;
                }
                SyncProgress::Done(note) => {
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
                SyncProgress::Failed(e) => {
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
        // Mientras el job siga vivo (`next` lo limpia solo si el worker murio), heartbeat throttled
        // (~7/seg): mantiene el loop y la velocidad/ETA al dia, y recoge el cierre del worker.
        if self.sync.apply_job.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }
}

pub(super) fn render_plan(ui: &mut egui::Ui, plan: &sync::Plan) {
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
        super::widgets::onboarding_load_order(ui);
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
                for d in &plan.to_download {
                    let tag = if d.is_delta() {
                        format!("  ·  delta {:.1} KB", d.fetch_bytes() as f64 / 1024.0)
                    } else {
                        String::new()
                    };
                    ui.label(format!(
                        "  + {}   ({:.1} KB){tag}",
                        d.entry.path,
                        d.entry.size as f64 / 1024.0
                    ));
                }
                if !plan.orphans.is_empty() {
                    ui.colored_label(BAD, format!("Huerfanos a borrar: {}", plan.orphans.len()));
                }
            });
    });
}
