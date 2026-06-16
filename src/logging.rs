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
    Some(crate::config::data_dir()?.join("sts2-modsync.log"))
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

/// Agrega una linea al log (best-effort), con prefijo de fecha/hora legible (UTC).
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
        let _ = writeln!(f, "[{}] {msg}", fmt_timestamp(epoch_secs()));
    }
}

/// `epoch_secs` -> `"YYYY-MM-DD HH:MM:SSZ"` (UTC). Para que el log sea legible por el amigo que lo
/// abre (antes era un epoch crudo, incorrelacionable). Algoritmo civil de Howard Hinnant (sin deps;
/// UTC para no depender del huso del SO — el sufijo `Z` lo deja claro).
fn fmt_timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // days-from-epoch (1970-01-01) -> fecha civil.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = era * 400 + yoe as i64 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}Z")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_timestamp_fechas_conocidas() {
        assert_eq!(fmt_timestamp(0), "1970-01-01 00:00:00Z");
        assert_eq!(fmt_timestamp(1_700_000_000), "2023-11-14 22:13:20Z");
        // un 29 de febrero (año bisiesto) para ejercitar el calendario.
        assert_eq!(fmt_timestamp(1_582_934_400), "2020-02-29 00:00:00Z");
    }
}
