//! Login de GitHub para publicar sin el `gh` CLI: card "Conectar con GitHub", validacion del
//! token guardado, conexion por PAT o por OAuth device-flow, desconexion y el poll de su worker.

use super::App;
use super::OK;
use super::widgets::card;
use eframe::egui;
use std::sync::mpsc::{TryRecvError, channel};

/// Eventos del worker de login de GitHub (PAT o device-flow).
pub(super) enum GhEvent {
    DeviceCode { user_code: String, uri: String }, // mostrar y abrir el navegador
    Connected(String),                             // login del usuario
    Disconnected,
    Failed(String),
}

/// Eventos del worker de repos de GitHub (listar los que podes pushear, o crear uno nuevo).
pub(super) enum GhRepoEvent {
    Listed(Vec<String>),
    Created(String), // "owner/repo" del repo creado/reusado
    Failed(String),
}

impl App {
    /// Valida UNA vez el token guardado en el llavero (whoami -> gh_user). Best-effort.
    pub(super) fn gh_check_stored(&mut self, ctx: &egui::Context) {
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

    pub(super) fn poll_gh_job(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.gh_job else {
            return;
        };
        match rx.try_recv() {
            Ok(GhEvent::DeviceCode { user_code, uri }) => {
                // Abrir el navegador automaticamente en la pagina de autorizacion (lo que promete el
                // flujo "log in con el navegador"); igual queda el link visible para re-abrir a mano.
                ctx.open_url(egui::OpenUrl::new_tab(&uri));
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

    /// Lista en un hilo los repos donde el usuario puede pushear (para elegir uno de publicacion).
    fn gh_load_repos(&mut self, ctx: &egui::Context) {
        if self.gh_repo_job.is_some() {
            return;
        }
        let Some(token) = crate::github::load_token() else {
            self.show_toast("conecta GitHub primero", true);
            return;
        };
        let (tx, rx) = channel();
        self.gh_repo_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let ev = match crate::github::Api::new(token).list_repos() {
                Ok(repos) => GhRepoEvent::Listed(repos),
                Err(e) => GhRepoEvent::Failed(format!("{e:#}")),
            };
            let _ = tx.send(ev);
            ctx.request_repaint();
        });
    }

    /// Crea en un hilo un repo PUBLICO nuevo (`gh_new_repo`) y lo deja elegido como repo de publicacion.
    fn gh_create_repo(&mut self, ctx: &egui::Context) {
        if self.gh_repo_job.is_some() {
            return;
        }
        let name = self.gh_new_repo.trim().to_string();
        if name.is_empty() {
            return;
        }
        let Some(token) = crate::github::load_token() else {
            self.show_toast("conecta GitHub primero", true);
            return;
        };
        let (tx, rx) = channel();
        self.gh_repo_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let ev = match crate::github::Api::new(token).create_repo(&name) {
                Ok(full) => GhRepoEvent::Created(full),
                Err(e) => GhRepoEvent::Failed(format!("{e:#}")),
            };
            let _ = tx.send(ev);
            ctx.request_repaint();
        });
    }

    pub(super) fn poll_gh_repo_job(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.gh_repo_job else {
            return;
        };
        match rx.try_recv() {
            Ok(GhRepoEvent::Listed(repos)) => {
                self.gh_repos = repos;
                self.gh_repo_job = None;
            }
            Ok(GhRepoEvent::Created(full)) => {
                self.gh_repo_job = None;
                self.gh_new_repo.clear();
                if !self.gh_repos.contains(&full) {
                    self.gh_repos.insert(0, full.clone());
                }
                self.select_publish_repo(full.clone());
                self.show_toast(format!("repo creado: {full}"), false);
            }
            Ok(GhRepoEvent::Failed(e)) => {
                self.gh_repo_job = None;
                self.show_toast(e, true);
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => self.gh_repo_job = None,
        }
    }

    /// Fija el repo de publicacion y lo RECUERDA (config.publish_repo) — al elegir/crear uno, no hace
    /// falta esperar a publicar para que quede guardado.
    fn select_publish_repo(&mut self, repo: String) {
        // Si cambia el repo y el usuario NO tipeo una version, re-proponer la siguiente para el repo
        // NUEVO (sino quedaria la propuesta del repo anterior). No pisar lo que el usuario escribio.
        if repo != self.pub_repo && self.pub_version.trim().is_empty() {
            self.pub_version_autofilled = false;
            self.pub_version_job = None;
        }
        self.pub_repo = repo.clone();
        self.cfg.publish_repo = Some(repo);
        let _ = crate::config::save(&self.cfg);
    }

    /// Elegir un repo existente (combo) o crear uno nuevo, sin tipear `owner/repo` a mano. Solo si
    /// hay sesion de GitHub (sino el campo de texto de la pestaña Publicar sigue sirviendo).
    pub(super) fn ui_gh_repo_picker(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.gh_user.is_none() && !crate::github::is_connected() {
            return;
        }
        let busy = self.gh_repo_job.is_some();
        ui.horizontal(|ui| {
            ui.label("Mis repos:");
            let mut chosen: Option<String> = None;
            egui::ComboBox::from_id_salt("gh_repo_combo")
                .selected_text(if self.pub_repo.is_empty() {
                    "elegir...".to_string()
                } else {
                    self.pub_repo.clone()
                })
                .show_ui(ui, |ui| {
                    if self.gh_repos.is_empty() {
                        ui.label(egui::RichText::new("(toca \"Cargar\")").weak());
                    }
                    for r in &self.gh_repos {
                        if ui.selectable_label(&self.pub_repo == r, r).clicked() {
                            chosen = Some(r.clone());
                        }
                    }
                });
            if let Some(r) = chosen {
                self.select_publish_repo(r);
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Cargar"))
                .on_hover_text("Lista tus repos de GitHub (los que podes pushear).")
                .clicked()
            {
                self.gh_load_repos(ctx);
            }
            if busy {
                ui.spinner();
            }
        });
        ui.horizontal(|ui| {
            ui.label("o crear repo:");
            ui.add(
                egui::TextEdit::singleline(&mut self.gh_new_repo)
                    .hint_text("nombre-del-repo")
                    .desired_width(200.0),
            );
            let can = !busy && !self.gh_new_repo.trim().is_empty();
            if ui
                .add_enabled(can, egui::Button::new("Crear (publico)"))
                .on_hover_text("Crea un repo PUBLICO nuevo bajo tu cuenta y lo deja elegido.")
                .clicked()
            {
                self.gh_create_repo(ctx);
            }
        });
    }

    /// Card "Conectar con GitHub" en la pestaña Publicar.
    pub(super) fn ui_github_connect(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
                        .hint_text("PAT classic con scope public_repo")
                        .desired_width(320.0),
                );
                if ui.button("Guardar token").clicked() {
                    self.gh_connect_pat(ctx);
                }
            });
            // Boton que ABRE EL NAVEGADOR en la pagina de creacion del token, con el scope correcto ya
            // preseleccionado: crear un PAT a mano (elegir scopes) es la parte confusa para no-expertos.
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("¿No tenes uno?").weak());
                if ui
                    .button("Abrir GitHub para crear el token")
                    .on_hover_text(
                        "Abre el navegador en GitHub con el scope public_repo ya marcado. Logueate, \
                         genera el token y pegalo arriba.",
                    )
                    .clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(
                        "https://github.com/settings/tokens/new?scopes=public_repo&description=sts2-modsync",
                    ));
                }
            });
            // "Log in con el navegador" de verdad (OAuth device-flow): abre GitHub, autorizas y listo,
            // sin pegar un token. Solo aparece si la app se compilo con un OAUTH_CLIENT_ID (registrar
            // una OAuth App en GitHub); sin el, queda el camino del PAT de arriba.
            if crate::github::device_flow_enabled()
                && ui.button("Log in con GitHub (abre el navegador)").clicked()
            {
                self.gh_connect_device(ctx);
            }
        });
        ui.add_space(6.0);
    }
}
