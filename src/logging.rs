//! Logging diagnostico a un archivo en %APPDATA% (el GUI puede no tener consola, y aunque la
//! tenga se cierra al crashear, perdiendo el mensaje). Incluye un panic-hook que vuelca el
//! panic + backtrace al log, asi un crash en produccion deja rastro. Todo best-effort: si no
//! se puede escribir, no rompe nada.

use std::io::Write;
use std::path::PathBuf;

/// Tope del log antes de rotar (evita que crezca sin limite). 1 MiB.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// Archivo de log: junto a la config, en `%APPDATA%/sts2-modsync/sts2-modsync.log`.
pub fn log_path() -> Option<PathBuf> {
    Some(
        crate::config::config_path()?
            .parent()?
            .join("sts2-modsync.log"),
    )
}

/// Inicializa el logging: rota si crecio mucho, instala el panic-hook y deja una linea de
/// arranque. Llamar UNA vez al inicio del binario que no tiene consola (el GUI).
pub fn init(context: &str) {
    rotate_if_big();
    install_panic_hook();
    log_line(&format!(
        "--- arranque {context} v{} ---",
        env!("CARGO_PKG_VERSION")
    ));
}

/// Agrega una linea al log (best-effort), con prefijo de epoch en segundos.
pub fn log_line(msg: &str) {
    let Some(path) = log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "[{}] {msg}", epoch_secs());
    }
}

/// Instala un panic-hook que ademas del comportamiento default vuelca el panic al log.
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        log_line(&format!("PANIC: {info}\nbacktrace:\n{bt}"));
        prev(info); // mantener el default (stderr si hay consola)
    }));
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Rota el log a `.log.old` si supero el tope (conserva una generacion anterior).
fn rotate_if_big() {
    let Some(path) = log_path() else {
        return;
    };
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_LOG_BYTES {
        let old = path.with_extension("log.old");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&path, &old);
    }
}
