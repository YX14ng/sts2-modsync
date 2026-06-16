//! Pestaña Perfiles: un perfil = un conjunto de mods habilitados. Guardar el set actual,
//! aplicar (deja exactamente esos) o borrar perfiles.

use super::App;
use crate::profile::{self, Profile};
use eframe::egui;

impl App {
    pub(super) fn ui_profiles(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
        let mut share: Option<Profile> = None;
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
                if ui.small_button("Compartir").clicked() {
                    share = Some(p.clone());
                }
                if ui.button("Borrar").clicked() {
                    delete = Some(p.name.clone());
                }
            });
        }
        if self.profiles.is_empty() {
            ui.label(egui::RichText::new("(todavia no hay perfiles guardados)").weak());
        }

        // --- Compartir / importar la lista por CODIGO -------------------------
        ui.separator();
        ui.label(egui::RichText::new("Compartir la lista por codigo").strong());
        ui.label(
            egui::RichText::new(
                "Genera un codigo de los mods ACTIVADOS ahora y pasaselo a un amigo (que YA tenga los \
                 mods): al pegarlo activa esos y desactiva el resto. No baja archivos.",
            )
            .weak(),
        );
        let mut gen_code = false;
        let mut copy_code = false;
        let mut apply_code = false;
        ui.horizontal(|ui| {
            if ui.button("Generar codigo de la lista actual").clicked() {
                gen_code = true;
            }
            if !self.share_code.is_empty() && ui.small_button("Copiar").clicked() {
                copy_code = true;
            }
        });
        if !self.share_code.is_empty() {
            // Campo selectable (solo-lectura efectivo: se regenera) para copiar a mano si hace falta.
            ui.add(
                egui::TextEdit::singleline(&mut self.share_code)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Pegar un codigo:");
            ui.add(
                egui::TextEdit::singleline(&mut self.import_code)
                    .hint_text("STS2L1...")
                    .desired_width(300.0),
            );
            let can = can_act && !self.import_code.trim().is_empty();
            if ui
                .add_enabled(can, egui::Button::new("Aplicar codigo"))
                .clicked()
            {
                apply_code = true;
            }
        });
        if self.game_running {
            ui.colored_label(
                super::WARN,
                "Cerra Slay the Spire 2 para aplicar una lista.",
            );
        }

        // Generar/compartir el codigo (de la lista actual o de un perfil) -> al portapapeles.
        if gen_code {
            let ids: Vec<String> = self
                .mods
                .iter()
                .filter(|m| m.enabled)
                .map(|m| m.id().to_string())
                .collect();
            self.share_code = crate::loadcode::encode("", &ids);
            copy_code = true;
            self.show_toast(
                format!("codigo copiado ({} mods activos)", ids.len()),
                false,
            );
        }
        if let Some(p) = share {
            self.share_code = crate::loadcode::encode(&p.name, &p.enabled_ids);
            copy_code = true;
            self.show_toast(format!("codigo de \"{}\" copiado", p.name), false);
        }
        if copy_code && !self.share_code.is_empty() {
            ctx.copy_text(self.share_code.clone());
        }
        if apply_code {
            match crate::loadcode::decode(&self.import_code) {
                Ok((name, ids)) => {
                    let install = self.install.clone().unwrap();
                    let prof = Profile {
                        name: if name.trim().is_empty() {
                            "codigo".into()
                        } else {
                            name
                        },
                        enabled_ids: ids,
                    };
                    self.import_code.clear();
                    self.run_action(ctx, "aplicando la lista del codigo...".into(), move || {
                        let r = profile::apply(&install, &prof)?;
                        let mut msg = format!(
                            "lista aplicada: +{} activados, -{} desactivados",
                            r.enabled.len(),
                            r.disabled.len()
                        );
                        if !r.not_installed.is_empty() {
                            msg.push_str(&format!(
                                " · faltan {} (no instalados): {}",
                                r.not_installed.len(),
                                r.not_installed.join(", ")
                            ));
                        }
                        Ok(msg)
                    });
                }
                Err(e) => self.show_toast(format!("codigo invalido: {e:#}"), true),
            }
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
