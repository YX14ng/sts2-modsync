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
}
