//! Pestaña Mods (el gestor): lista con filtro/orden, warnings agregados (deps/conflictos/orden
//! de carga), detalle del mod seleccionado y las acciones enable/disable/install/uninstall.

use super::widgets::{card, human_size, mod_matches, onboarding_load_order};
use super::{ACCENT, App, BAD, WARN};
use crate::modsource::ModSource;
use crate::{manager, modlist};
use eframe::egui;
use std::sync::mpsc::{TryRecvError, channel};

impl App {
    pub(super) fn ui_mods(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
            // Canal GLOBAL de actualizacion de mods (estable vs beta/pre-releases).
            ui.separator();
            if ui
                .checkbox(&mut self.cfg.prefer_beta, "Canal beta")
                .on_hover_text("Seguir versiones BETA (pre-releases) al actualizar mods. Sin tildar: solo estables (MAIN).")
                .changed()
            {
                let _ = crate::config::save(&self.cfg);
                self.mod_updates.clear(); // el canal cambio: invalidar lo encontrado
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
            // Cambio de mod -> limpiar el campo de origen (es uno solo, compartido): asi no se
            // guarda por error el origen tipeado para un mod contra OTRO recien seleccionado.
            if self.selected.as_deref() != Some(id.as_str()) {
                self.mod_source_input.clear();
            }
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

        // --- Actualizaciones (upstream del mod) ---------------------------------
        ui.add_space(6.0);
        ui.separator();
        let source = crate::modupdate::effective_source(&m, &self.cfg);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Origen:").strong());
            match &source {
                Some(src) => {
                    ui.label(src.label());
                    ui.hyperlink_to("abrir", src.web_url());
                }
                None => {
                    ui.label(egui::RichText::new("sin definir — pegalo abajo").weak());
                }
            }
        });
        // Pegar/cambiar el origen (se recuerda en config.mod_sources).
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.mod_source_input)
                    .hint_text("usuario/repo  o  URL de Nexus")
                    .desired_width(280.0),
            );
            let parsed = crate::modsource::ModSource::parse(&self.mod_source_input);
            if ui
                .add_enabled(parsed.is_some(), egui::Button::new("Guardar origen"))
                .clicked()
                && let Some(src) = parsed
            {
                self.cfg.mod_sources.insert(id.clone(), src.to_storage());
                let _ = crate::config::save(&self.cfg);
                self.mod_source_input.clear();
                self.mod_updates.remove(&id);
            }
        });
        // Chequear / actualizar segun el tipo de origen.
        if let Some(src) = &source {
            if src.supports_auto_download() {
                ui.horizontal(|ui| {
                    let checking = self.mod_update_job.is_some();
                    if ui
                        .add_enabled(!checking, egui::Button::new("Buscar actualizacion"))
                        .clicked()
                    {
                        self.check_mod_update(ctx, id.clone(), src.clone());
                    }
                    if checking {
                        ui.spinner();
                    }
                });
                // Solo una entrada CON asset (GitHub); una stale de Nexus (asset_url vacio) no se
                // puede "Actualizar" desde aca (su descarga es 2b).
                if let Some(upd) = self
                    .mod_updates
                    .get(&id)
                    .filter(|u| !u.asset_url.is_empty())
                    .cloned()
                {
                    let chan = if upd.prerelease { "beta" } else { "estable" };
                    ui.colored_label(
                        ACCENT,
                        format!(
                            "● v{} disponible ({chan}) · tenes v{}",
                            upd.latest,
                            upd.current.as_deref().unwrap_or("?")
                        ),
                    );
                    if ui
                        .add_enabled(can_act, egui::Button::new("Actualizar a esta version"))
                        .clicked()
                    {
                        self.apply_mod_update(ctx, id.clone(), upd);
                    } else if !can_act && self.game_running {
                        ui.colored_label(WARN, "Cerra Slay the Spire 2 para actualizar.");
                    }
                }
            } else {
                // Nexus: chequeo de version via API (fase 2a); la descarga auto (handler nxm://) es 2b.
                if self.nexus_connected {
                    ui.horizontal(|ui| {
                        let checking = self.mod_update_job.is_some();
                        if ui
                            .add_enabled(!checking, egui::Button::new("Buscar actualizacion"))
                            .clicked()
                        {
                            self.check_mod_update(ctx, id.clone(), src.clone());
                        }
                        if checking {
                            ui.spinner();
                        }
                        if let Some(name) = &self.nexus_user {
                            ui.label(egui::RichText::new(format!("Nexus: {name}")).weak());
                        }
                        if ui.small_button("desconectar").clicked() {
                            self.nexus_disconnect();
                        }
                    });
                    if let Some(upd) = self.mod_updates.get(&id) {
                        ui.colored_label(
                            ACCENT,
                            format!(
                                "● v{} disponible (Nexus) · tenes v{}",
                                upd.latest,
                                upd.current.as_deref().unwrap_or("?")
                            ),
                        );
                    }
                    ui.horizontal(|ui| {
                        ui.hyperlink_to("Abrir en Nexus para bajar", src.web_url());
                        ui.label(
                            egui::RichText::new("(descarga automatica de Nexus: fase 2b)").weak(),
                        );
                    });
                } else {
                    // Sin API key: pegar para conectar y poder chequear versiones de Nexus.
                    ui.label(
                        egui::RichText::new(
                            "Conecta tu API Key de Nexus para chequear versiones (Preferences -> API en tu cuenta):",
                        )
                        .weak(),
                    );
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.nexus_key_input)
                                .hint_text("API Key de Nexus")
                                .password(true)
                                .desired_width(280.0),
                        );
                        let busy = self.nexus_job.is_some();
                        if ui
                            .add_enabled(
                                !busy && !self.nexus_key_input.trim().is_empty(),
                                egui::Button::new("Conectar"),
                            )
                            .clicked()
                        {
                            self.nexus_connect(ctx);
                        }
                        if busy {
                            ui.spinner();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.hyperlink_to("Abrir en Nexus para bajar", src.web_url());
                    });
                }
                // Handler nxm://: con esto, "Mod Manager Download" en Nexus baja+instala con esta app.
                // Se lee directo (no cacheado): asi refleja un cambio hecho por CLI o por otra app.
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if crate::nxm::is_registered() {
                        ui.colored_label(super::OK, "✓ handler nxm:// registrado");
                        ui.label(
                            egui::RichText::new("(toca \"Mod Manager Download\" en Nexus)").weak(),
                        );
                        if ui.small_button("quitar").clicked() {
                            match crate::nxm::unregister() {
                                Ok(()) => self.show_toast("handler nxm:// removido", false),
                                Err(e) => self.show_toast(format!("{e:#}"), true),
                            }
                        }
                    } else if ui
                        .button("Registrar \"Mod Manager Download\" (nxm://)")
                        .on_hover_text(
                            "Registra esta app como handler de nxm://: al tocar \"Mod Manager Download\" \
                             en la web de Nexus, baja e instala el mod aca. Toma el protocolo de Vortex/MO2 \
                             (lo respalda y lo restaura si lo quitas).",
                        )
                        .clicked()
                    {
                        match crate::nxm::register() {
                            Ok(()) => self.show_toast("handler nxm:// registrado", false),
                            Err(e) => self.show_toast(format!("{e:#}"), true),
                        }
                    }
                });
            }
        }
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

    // --- auto-update de mods (upstream GitHub) ------------------------------

    /// Chequea en un hilo si hay version nueva de `id` en su origen (GitHub por canal, o Nexus via API).
    fn check_mod_update(&mut self, ctx: &egui::Context, id: String, src: ModSource) {
        let current = self
            .mods
            .iter()
            .find(|m| m.id() == id)
            .and_then(|m| m.manifest.version.clone());
        let installed_tag = self.cfg.mod_installed_tag.get(&id).cloned();
        let prefer_beta = self.cfg.prefer_beta;
        let (tx, rx) = channel();
        self.mod_update_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let res = match src {
                ModSource::GitHub { owner, repo } => crate::modupdate::check_github(
                    &owner,
                    &repo,
                    &id,
                    current.as_deref(),
                    installed_tag.as_deref(),
                    prefer_beta,
                ),
                ModSource::Nexus { game, mod_id } => {
                    crate::modupdate::check_nexus(&id, &game, mod_id, current.as_deref())
                }
            }
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send((id, res));
            ctx.request_repaint();
        });
    }

    /// Conecta la API Key de Nexus: la guarda en el llavero y la valida en un hilo.
    fn nexus_connect(&mut self, ctx: &egui::Context) {
        let key = self.nexus_key_input.trim().to_string();
        if key.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        self.nexus_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let res = (|| -> std::result::Result<String, String> {
                crate::nexus::store_key(&key).map_err(|e| format!("{e:#}"))?;
                let user = crate::nexus::validate().map_err(|e| {
                    // key invalida: no dejarla guardada.
                    let _ = crate::nexus::clear_key();
                    format!("{e:#}")
                })?;
                Ok(user.name)
            })();
            let _ = tx.send(res);
            ctx.request_repaint();
        });
    }

    /// Desconecta Nexus: borra la API key del llavero y limpia el cache de conexion.
    fn nexus_disconnect(&mut self) {
        let _ = crate::nexus::clear_key();
        self.nexus_connected = false;
        self.nexus_user = None;
        self.show_toast("desconectado de Nexus", false);
    }

    pub(super) fn poll_nexus_job(&mut self) {
        let Some(rx) = &self.nexus_job else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(name)) => {
                self.nexus_user = Some(name.clone());
                self.nexus_connected = true;
                self.nexus_key_input.clear();
                self.nexus_job = None;
                self.show_toast(format!("Nexus conectado como {name}"), false);
            }
            Ok(Err(e)) => {
                self.nexus_user = None;
                self.nexus_job = None;
                self.show_toast(e, true);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.nexus_job = None,
        }
    }

    pub(super) fn poll_mod_update(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.mod_update_job else {
            return;
        };
        match rx.try_recv() {
            Ok((id, Ok(Some(upd)))) => {
                self.mod_updates.insert(id, upd);
                self.mod_update_job = None;
            }
            Ok((id, Ok(None))) => {
                self.mod_updates.remove(&id);
                self.show_toast(format!("{id}: ya estas en la ultima version"), false);
                self.mod_update_job = None;
            }
            Ok((_, Err(e))) => {
                self.show_toast(e, true);
                self.mod_update_job = None;
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => self.mod_update_job = None,
        }
    }

    /// Baja e instala la version nueva (reemplaza solo si el zip es ese mod, preserva enable/disable,
    /// recuerda el tag). Re-escanea al terminar; `poll_action` recarga la config (toma el tag nuevo).
    fn apply_mod_update(
        &mut self,
        ctx: &egui::Context,
        id: String,
        upd: crate::modupdate::ModUpdate,
    ) {
        let Some(install) = self.install.clone() else {
            return;
        };
        self.mod_updates.remove(&id);
        self.run_action(ctx, format!("actualizando {id}..."), move || {
            crate::modupdate::apply(&install, &upd.mod_id, &upd.asset_url, &upd.tag)?;
            Ok(format!("actualizado: {id} v{}", upd.latest))
        });
    }
}
