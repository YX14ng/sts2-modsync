//! sts2-modsync — biblioteca del sincronizador de mods de Slay the Spire 2.
//!
//! El binario (`main.rs`) es una cascara fina sobre estos modulos. El core es
//! agnostico de GUI/red: detecta el install, modela el manifiesto, hashea, y
//! calcula el PLAN de sincronizacion. La descarga (transport) y la UI (eframe)
//! son capas de FASE 2 — ver HANDOFF.md.

pub mod config;
pub mod detect;
pub mod hashing;
pub mod manifest;
pub mod signing;
pub mod sync;
pub mod transport;

/// AppID de Slay the Spire 2 en Steam (verificado: SteamDB + appmanifest_2868840.acf).
pub const STS2_STEAM_APPID: u32 = 2868840;
