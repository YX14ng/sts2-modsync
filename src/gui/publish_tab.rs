//! Pestaña Publicar (lado modder): genera el set-manifest + assets desde los mods elegidos
//! (habilitados o un perfil) y los sube a un GitHub Release. Incluye el control de seeding P2P
//! del set ya publicado. La card "Conectar con GitHub" vive en `github_login`.

use super::App;
use crate::{config, manager, modlist, profile, publish};
use eframe::egui;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::WARN;

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

    pub(super) fn ui_publish(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
                let mut prep = publish::prepare(&mods, &ids, &params)?;
                // Delta intra-archivo contra la publicacion anterior en out_dir (best-effort: si
                // falla, se publica sin deltas). Los amigos al dia bajan solo el diff.
                let deltas = publish::add_deltas(&mut prep, &out_dir).unwrap_or_default();
                publish::write_out(&prep, &out_dir)?;
                let url = publish::upload(&out_dir, &params.base_url)?;
                let extra = if deltas.patches > 0 {
                    format!(
                        " · {} delta(s) ({:.1} MB)",
                        deltas.patches,
                        deltas.patch_bytes as f64 / 1.0e6
                    )
                } else {
                    String::new()
                };
                Ok(format!(
                    "Publicado: {} assets ({:.1} MB){extra} → {url}",
                    prep.assets.len(),
                    prep.total_bytes() as f64 / 1.0e6,
                ))
            },
        );
    }
}
