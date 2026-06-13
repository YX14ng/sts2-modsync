//! sts2-modsync — biblioteca del sincronizador de mods de Slay the Spire 2.
//!
//! El binario (`main.rs`) es una cascara fina sobre estos modulos. El core es
//! agnostico de GUI/red: detecta el install, modela el manifiesto, hashea, y
//! calcula el PLAN de sincronizacion. La descarga (transport) y la UI (eframe)
//! son capas de FASE 2 — ver HANDOFF.md.

pub mod config;
pub mod detect;
pub mod hashing;
pub mod launch;
pub mod manager;
pub mod manifest;
pub mod modlist;
pub mod profile;
pub mod publish;
pub mod signing;
pub mod sync;
pub mod transport;
pub mod update;

/// Front-end GUI (eframe/egui) de FASE 2. Opcional: solo se compila con `--features gui`
/// para no inflar el build del core ni de la CLI. Reusa todos los modulos de arriba.
#[cfg(feature = "gui")]
pub mod gui;

/// AppID de Slay the Spire 2 en Steam (verificado: SteamDB + appmanifest_2868840.acf).
pub const STS2_STEAM_APPID: u32 = 2868840;
