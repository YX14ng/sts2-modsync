//! Punto de entrada del binario GUI (`sts2-modsync-gui`), gateado tras el feature
//! `gui` en Cargo.toml. Es una cascara: toda la UI vive en `sts2_modsync::gui`.

fn main() {
    if let Err(e) = sts2_modsync::gui::run() {
        eprintln!("sts2-modsync-gui: error fatal: {e}");
        std::process::exit(1);
    }
}
