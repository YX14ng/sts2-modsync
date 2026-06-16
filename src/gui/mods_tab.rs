//! Pestaña Mods (el gestor): lista con filtro/orden, warnings agregados (deps/conflictos/orden
//! de carga), detalle del mod seleccionado y las acciones enable/disable/install/uninstall.

use super::widgets::{card, human_size, mod_matches, onboarding_load_order};
use super::{ACCENT, App, BAD, WARN};
use crate::modsource::ModSource;
use crate::{manager, modlist};
use eframe::egui;
use std::sync::mpsc::{TryRecvError, channel};

/// Resultado del worker de Nexus (conectar la key, o validar la guardada al arrancar). `announce`
/// distingue una conexion EXPLICITA del usuario (toast en exito/error) de la validacion silenciosa
/// de arranque (no molestar con un toast si hay un blip de red).
pub(super) enum NexusEvent {
    Connected {
        name: String,
        premium: bool,
        announce: bool,
    },
    Failed {
        msg: String,
        announce: bool,
    },
}

impl App {
    pub(super) fn ui_mods(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.install.is_none() {
            ui.label("Detecta o elegi la carpeta del juego (arriba) para ver los mods.");
            return;
        }
        // Validar una vez la key de Nexus guardada (saber si la cuenta es Premium -> descarga directa).
        self.nexus_check_stored(ctx);

        let mut pick_dir = false;
        let mut pick_zip = false;
        let mut open_mods_folder = false;
        let mut open_data_folder = false;
        let mut copy_diag = false;
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
            if ui
                .button("Abrir carpeta de mods")
                .on_hover_text("Abre mods/ en el explorador (para instalar/revisar a mano).")
                .clicked()
            {
                open_mods_folder = true;
            }
            if ui
                .button("Abrir datos/log")
                .on_hover_text(
                    "Abre la carpeta de config y log (%APPDATA%/sts2-modsync) — util si un error \
                     te pide revisar el log.",
                )
                .clicked()
            {
                open_data_folder = true;
            }
            if ui
                .button("Copiar diagnostico")
                .on_hover_text(
                    "Copia un bloque con tu estado (version, ModListSorter, huella de orden de carga, \
                     mods+versiones) para pegar cuando \"no podemos jugar juntos\".",
                )
                .clicked()
            {
                copy_diag = true;
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
        if open_mods_folder
            && let Some(install) = &self.install
            && let Err(e) = manager::open_folder(&install.mods_dir)
        {
            self.show_toast(format!("no se pudo abrir la carpeta: {e:#}"), true);
        }
        if open_data_folder {
            match crate::config::data_dir() {
                Some(dir) => {
                    if let Err(e) = manager::open_folder(&dir) {
                        self.show_toast(format!("no se pudo abrir la carpeta: {e:#}"), true);
                    }
                }
                None => self.show_toast("no se pudo resolver la carpeta de datos", true),
            }
        }
        if copy_diag && let Some(install) = &self.install {
            let report = crate::doctor::report(install, &self.mods, &self.cfg);
            ctx.copy_text(report);
            self.show_toast("diagnostico copiado al portapapeles", false);
        }

        // Chequear updates de TODOS los mods de una (varias llamadas a la red en un worker).
        let mut check_all = false;
        ui.horizontal(|ui| {
            let checking = self.mod_update_all_job.is_some();
            if ui
                .add_enabled(
                    !checking,
                    egui::Button::new("Buscar actualizaciones de todos los mods"),
                )
                .on_hover_text(
                    "Revisa el upstream de cada mod con origen conocido. Sin login de GitHub, el \
                     limite anonimo (60/h) puede no alcanzar para muchos mods.",
                )
                .clicked()
            {
                check_all = true;
            }
            if checking {
                ui.spinner();
                ui.label(egui::RichText::new("chequeando todos...").weak());
            } else {
                let n = self.mod_updates.len();
                if n > 0 {
                    ui.colored_label(ACCENT, format!("● {n} con actualizacion"));
                }
            }
        });
        if check_all {
            self.check_all_updates(ctx);
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
            // Limpiar de una: por cada id duplicado deja la version MAS NUEVA y manda las otras a la
            // papelera (reversible). Calculado aca; `dedupe_mods` lo recomputa al ejecutar.
            let n_dup: usize = modlist::duplicates(&self.mods)
                .iter()
                .map(|g| g.remove.len())
                .sum();
            if n_dup > 0 {
                let can = self.busy.is_empty() && self.action_job.is_none() && !self.game_running;
                let label =
                    format!("Quitar {n_dup} duplicado(s) — deja la version mas nueva (papelera)");
                if ui.add_enabled(can, egui::Button::new(label)).clicked() {
                    self.dedupe_mods(ctx);
                }
                if self.game_running {
                    ui.colored_label(WARN, "Cerra Slay the Spire 2 para limpiar duplicados.");
                }
            }
        }
        // Huella del orden de carga: un valor CONCRETO para comparar con un amigo (misma huella =
        // mismo orden = mismo lobby). Mas util que la lista cruda para confirmar el match.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Huella de orden de carga:").strong());
            let fp = modlist::current_fingerprint(&self.mods);
            ui.add(egui::Label::new(
                egui::RichText::new(&fp).monospace().color(ACCENT),
            ))
            .on_hover_text(
                "Compartila con tu amigo: si los dos ven la MISMA huella, tienen el mismo orden \
                     de carga y entran al mismo lobby. (Comparar codigos: pestaña Perfiles.)",
            );
        });
        // En acento (azul) para que se note: es una linea de info larga que conviene distinguir.
        ui.colored_label(
            ACCENT,
            format!(
                "Orden de carga (multiplayer): {}",
                modlist::load_order(&self.mods).join("  →  ")
            ),
        );
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

        // Acciones masivas (util cuando hay muchos mods: troubleshooting, cambiar de contexto). Operan
        // sobre TODOS los mods (no el filtro), son reversibles (mover carpetas) y toleran fallos por
        // mod (reportan cuantos no se pudieron). Para "dejar exactamente este set" estan los Perfiles.
        let mut enable_all = false;
        let mut disable_all = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(can_act && n_off > 0, egui::Button::new("Habilitar todos"))
                .clicked()
            {
                enable_all = true;
            }
            if ui
                .add_enabled(can_act && n_on > 0, egui::Button::new("Deshabilitar todos"))
                .clicked()
            {
                disable_all = true;
            }
            if self.game_running {
                ui.colored_label(WARN, "Cerra el juego para habilitar/deshabilitar.");
            }
        });
        if enable_all {
            self.bulk_toggle(ctx, true);
        }
        if disable_all {
            self.bulk_toggle(ctx, false);
        }

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
                            // ¿Hay una actualizacion hallada para este mod? (marca de la lista).
                            let has_update = self.mod_updates.contains_key(id.as_str());

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
                                if has_update {
                                    ui.colored_label(ACCENT, "● update").on_hover_text(
                                        "Hay una version nueva — abri el detalle para actualizar.",
                                    );
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
                // Nexus: chequeo de version via API. Premium -> descarga DIRECTA; gratis -> nxm://.
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
                            let tag = if self.nexus_premium { " (Premium)" } else { "" };
                            ui.label(egui::RichText::new(format!("Nexus: {name}{tag}")).weak());
                        }
                        if ui.small_button("desconectar").clicked() {
                            self.nexus_disconnect();
                        }
                    });
                    if let Some(upd) = self.mod_updates.get(&id).cloned() {
                        ui.colored_label(
                            ACCENT,
                            format!(
                                "● v{} disponible (Nexus) · tenes v{}",
                                upd.latest,
                                upd.current.as_deref().unwrap_or("?")
                            ),
                        );
                        if upd.nexus.is_some() {
                            // Premium: descarga e instalacion directa (sin handler nxm://).
                            if ui
                                .add_enabled(can_act, egui::Button::new("Actualizar (Premium)"))
                                .clicked()
                            {
                                self.apply_nexus_update(ctx, id.clone(), upd);
                            } else if !can_act && self.game_running {
                                ui.colored_label(WARN, "Cerra Slay the Spire 2 para actualizar.");
                            }
                        } else {
                            // Cuenta gratis: la descarga directa no aplica -> "Mod Manager Download" o a mano.
                            ui.horizontal(|ui| {
                                ui.hyperlink_to("Abrir en Nexus para bajar", src.web_url());
                                ui.label(
                                    egui::RichText::new(
                                        "(cuenta gratis: usa \"Mod Manager Download\" en Nexus, o conecta una Premium)",
                                    )
                                    .weak(),
                                );
                            });
                        }
                    } else {
                        ui.hyperlink_to("Abrir en Nexus", src.web_url());
                    }
                } else {
                    // Sin API key: pegar para conectar y poder chequear versiones de Nexus.
                    ui.label(
                        egui::RichText::new(
                            "Conecta tu API Key de Nexus para chequear versiones. Sacala de tu cuenta \
                             (Preferences -> API Keys); el boton de abajo abre esa pagina:",
                        )
                        .weak(),
                    );
                    // Boton que ABRE EL NAVEGADOR en la pagina de API Keys de Nexus (antes solo decia
                    // "Preferences -> API" como texto y habia que buscarla a mano).
                    if ui
                        .button("Abrir Nexus para sacar mi API Key")
                        .on_hover_text(
                            "Abre la pagina de API Keys de tu cuenta de Nexus en el navegador. Logueate, \
                             genera/copia tu \"Personal API Key\" y pegala abajo.",
                        )
                        .clicked()
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(
                            "https://www.nexusmods.com/users/myaccount?tab=api",
                        ));
                    }
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

    /// Limpia los mods DUPLICADOS (mismo id en >1 carpeta): por cada grupo conserva la version mas
    /// nueva y manda las demas a la papelera (reversible). Re-escanea al terminar.
    fn dedupe_mods(&mut self, ctx: &egui::Context) {
        let Some(install) = self.install.clone() else {
            return;
        };
        let groups = modlist::duplicates(&self.mods);
        if groups.is_empty() {
            return;
        }
        self.run_action(ctx, "quitando duplicados...".into(), move || {
            // Continuar ante un fallo (carpeta ya borrada afuera, etc.): un item trabado no debe
            // bloquear limpiar el resto. Cada `trash_mod_dir` re-chequea juego-cerrado + ubicacion.
            let mut removed = 0usize;
            let mut failed = 0usize;
            for g in &groups {
                for m in &g.remove {
                    match manager::trash_mod_dir(&install, &m.dir) {
                        Ok(()) => removed += 1,
                        Err(_) => failed += 1,
                    }
                }
            }
            if failed > 0 {
                Ok(format!(
                    "quitados {removed} duplicado(s) a la papelera; {failed} no se pudieron (reintenta)"
                ))
            } else {
                Ok(format!(
                    "quitados {removed} duplicado(s) a la papelera (se conservo la version mas nueva)"
                ))
            }
        });
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
    /// Habilita (`enable=true`) o deshabilita TODOS los mods en una sola accion. Solo toca los que
    /// hay que cambiar (ids DISTINTOS) y TOLERA fallos por mod (una carpeta con nombre != id, o un
    /// duplicado, puede fallar el move por id): los reporta, no aborta el resto. Reversible.
    fn bulk_toggle(&mut self, ctx: &egui::Context, enable: bool) {
        let Some(install) = self.install.clone() else {
            return;
        };
        let ids: std::collections::BTreeSet<String> = self
            .mods
            .iter()
            .filter(|m| m.enabled != enable)
            .map(|m| m.id().to_string())
            .collect();
        if ids.is_empty() {
            return;
        }
        let verb = if enable {
            "habilitando"
        } else {
            "deshabilitando"
        };
        self.run_action(ctx, format!("{verb} todos..."), move || {
            let (mut n, mut failed) = (0usize, 0usize);
            for id in &ids {
                let r = if enable {
                    manager::enable(&install, id)
                } else {
                    manager::disable(&install, id)
                };
                if r.is_ok() { n += 1 } else { failed += 1 }
            }
            let done = if enable { "habilitados" } else { "deshabilitados" };
            Ok(if failed == 0 {
                format!("{done} {n} mod(s)")
            } else {
                format!(
                    "{done} {n} mod(s) · {failed} no se pudieron (carpeta con nombre != id o duplicado)"
                )
            })
        });
    }

    /// Contexto del chequeo de updates a partir del estado actual (canal global + cuentas de Nexus).
    fn check_ctx(&self) -> crate::modupdate::CheckCtx {
        crate::modupdate::CheckCtx {
            prefer_beta: self.cfg.prefer_beta,
            nexus_connected: self.nexus_connected,
            nexus_premium: self.nexus_premium,
        }
    }

    fn check_mod_update(&mut self, ctx: &egui::Context, id: String, src: ModSource) {
        let current = self
            .mods
            .iter()
            .find(|m| m.id() == id)
            .and_then(|m| m.manifest.version.clone());
        let installed_tag = self.cfg.mod_installed_tag.get(&id).cloned();
        let cctx = self.check_ctx();
        let (tx, rx) = channel();
        self.mod_update_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let res = crate::modupdate::check(
                &src,
                &id,
                current.as_deref(),
                installed_tag.as_deref(),
                cctx,
            )
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send((id, res));
            ctx.request_repaint();
        });
    }

    /// Chequea de UNA si hay version nueva de TODOS los mods con origen conocido. Pesa varias
    /// llamadas a la red (una por mod), por eso corre en un worker; reporta cuantos NO se pudieron
    /// chequear (rate-limit de GitHub sin login, sin conexion). Los de Nexus se saltean si no hay
    /// API key conectada (no se cuentan como fallo).
    fn check_all_updates(&mut self, ctx: &egui::Context) {
        if self.mod_update_all_job.is_some() {
            return;
        }
        let mods = self.mods.clone();
        let cfg = self.cfg.clone();
        let cctx = self.check_ctx();
        let (tx, rx) = channel();
        self.mod_update_all_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut found = std::collections::HashMap::new();
            let mut failed = 0usize;
            for m in &mods {
                let Some(src) = crate::modupdate::effective_source(m, &cfg) else {
                    continue;
                };
                let current = m.manifest.version.as_deref();
                let installed_tag = cfg.mod_installed_tag.get(m.id()).map(String::as_str);
                // `check` saltea Nexus sin conexion devolviendo Ok(None) (no cuenta como fallo).
                match crate::modupdate::check(&src, m.id(), current, installed_tag, cctx) {
                    Ok(Some(upd)) => {
                        found.insert(m.id().to_string(), upd);
                    }
                    Ok(None) => {}
                    Err(_) => failed += 1,
                }
            }
            let _ = tx.send((found, failed));
            ctx.request_repaint();
        });
    }

    pub(super) fn poll_mod_update_all(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.mod_update_all_job else {
            return;
        };
        match rx.try_recv() {
            Ok((found, failed)) => {
                let n = found.len();
                for (id, upd) in found {
                    self.mod_updates.insert(id, upd);
                }
                self.mod_update_all_job = None;
                let mut msg = if n == 0 {
                    "todos los mods estan al dia".to_string()
                } else {
                    format!("{n} mod(s) con actualizacion (marcados con ● update)")
                };
                if failed > 0 {
                    msg.push_str(&format!(
                        " · {failed} no se pudieron chequear (¿rate-limit de GitHub? conecta GitHub para 5000/h)"
                    ));
                }
                self.show_toast(msg, failed > 0 && n == 0);
            }
            Err(TryRecvError::Empty) => ctx.request_repaint(),
            Err(TryRecvError::Disconnected) => self.mod_update_all_job = None,
        }
    }

    /// Valida UNA vez la API key guardada (whoami -> nombre + Premium). Best-effort y SILENCIOSO: si
    /// falla (blip de red), no molesta con un toast y deja la key guardada. Asi al abrir la app ya
    /// sabemos si la cuenta es Premium (habilita la descarga directa).
    pub(super) fn nexus_check_stored(&mut self, ctx: &egui::Context) {
        if self.nexus_checked || self.nexus_job.is_some() {
            return;
        }
        self.nexus_checked = true;
        if !crate::nexus::is_connected() {
            return;
        }
        let (tx, rx) = channel();
        self.nexus_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let ev = match crate::nexus::validate() {
                Ok(u) => NexusEvent::Connected {
                    name: u.name,
                    premium: u.is_premium,
                    announce: false,
                },
                Err(e) => NexusEvent::Failed {
                    msg: format!("{e:#}"),
                    announce: false,
                },
            };
            let _ = tx.send(ev);
            ctx.request_repaint();
        });
    }

    /// Conecta la API Key de Nexus: la guarda en el llavero y la valida en un hilo (toast al terminar).
    fn nexus_connect(&mut self, ctx: &egui::Context) {
        let key = self.nexus_key_input.trim().to_string();
        if key.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        self.nexus_job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let ev = match crate::nexus::store_key(&key).and_then(|()| crate::nexus::validate()) {
                Ok(u) => NexusEvent::Connected {
                    name: u.name,
                    premium: u.is_premium,
                    announce: true,
                },
                Err(e) => {
                    // key invalida o no guardable: no dejar una key que no valida en el llavero.
                    let _ = crate::nexus::clear_key();
                    NexusEvent::Failed {
                        msg: format!("{e:#}"),
                        announce: true,
                    }
                }
            };
            let _ = tx.send(ev);
            ctx.request_repaint();
        });
    }

    /// Desconecta Nexus: borra la API key del llavero y limpia el cache de conexion.
    fn nexus_disconnect(&mut self) {
        let _ = crate::nexus::clear_key();
        self.nexus_connected = false;
        self.nexus_user = None;
        self.nexus_premium = false;
        self.show_toast("desconectado de Nexus", false);
    }

    pub(super) fn poll_nexus_job(&mut self) {
        let Some(rx) = &self.nexus_job else {
            return;
        };
        match rx.try_recv() {
            Ok(NexusEvent::Connected {
                name,
                premium,
                announce,
            }) => {
                self.nexus_user = Some(name.clone());
                self.nexus_premium = premium;
                self.nexus_connected = true;
                self.nexus_key_input.clear();
                self.nexus_job = None;
                if announce {
                    let tag = if premium { " (Premium)" } else { "" };
                    self.show_toast(format!("Nexus conectado como {name}{tag}"), false);
                }
            }
            Ok(NexusEvent::Failed { msg, announce }) => {
                self.nexus_job = None;
                if announce {
                    // Conexion explicita fallida: la key ya se borro en el worker.
                    self.nexus_user = None;
                    self.nexus_connected = false;
                    self.show_toast(msg, true);
                }
                // Validacion de arranque fallida (announce=false): dejar el estado como esta.
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

    /// Baja e instala DIRECTO de Nexus (Premium): resuelve el download-link del archivo MAIN, baja
    /// el `.zip` e instala reemplazando solo si el zip es ese mod (preserva enable/disable).
    fn apply_nexus_update(
        &mut self,
        ctx: &egui::Context,
        id: String,
        upd: crate::modupdate::ModUpdate,
    ) {
        let Some(install) = self.install.clone() else {
            return;
        };
        let Some(nref) = upd.nexus.clone() else {
            return;
        };
        self.mod_updates.remove(&id);
        self.run_action(
            ctx,
            format!("actualizando {id} desde Nexus..."),
            move || {
                crate::modupdate::apply_nexus(&install, &upd.mod_id, &nref, &upd.latest)?;
                Ok(format!("actualizado desde Nexus: {id} v{}", upd.latest))
            },
        );
    }
}
