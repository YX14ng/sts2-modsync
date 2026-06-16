//! Helpers chicos compartidos por varios modulos (formato de tamaños, nombres temporales unicos),
//! para no tener N copias que despues divergen.

/// Tamaño legible en binario (1 KB = 1024 B). `space` mete un espacio antes de la unidad: `true`
/// da `1.5 MB` (para la GUI), `false` da `1.5MB` (mas compacto, para la salida de la CLI).
pub fn human_size(bytes: u64, space: bool) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    let sep = if space { " " } else { "" };
    if bytes >= MB {
        format!("{:.1}{sep}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0}{sep}KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes}{sep}B")
    }
}

/// Nanosegundos desde el epoch — para nombres de archivo/carpeta temporales unicos. `0` si el reloj
/// quedo antes del epoch (no deberia). NO es criptografico ni garantiza unicidad bajo llamadas
/// rapidisimas; alcanza para los temp de uso puntual (extraer un zip, bajar un asset).
pub fn unique_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_binario_y_separador() {
        assert_eq!(human_size(512, false), "512B");
        assert_eq!(human_size(2048, false), "2KB");
        assert_eq!(human_size(2048, true), "2 KB");
        assert_eq!(human_size(1024 * 1024 * 3 / 2, false), "1.5MB");
        assert_eq!(human_size(1024 * 1024 * 3 / 2, true), "1.5 MB");
    }
}
