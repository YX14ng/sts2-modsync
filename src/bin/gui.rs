//! Punto de entrada del binario GUI (`sts2-modsync-gui`), gateado tras el feature
//! `gui` en Cargo.toml. Es una cascara: toda la UI vive en `sts2_modsync::gui`.

fn main() {
    // Self-test del auto-update: arrancar y salir 0 SIN abrir ventana. Verifica que el binario
    // nuevo es ejecutable/compatible antes de que `update::apply` lo relance de verdad.
    if std::env::args().skip(1).any(|a| a == "--health-check") {
        println!("sts2-modsync-gui {} OK", env!("CARGO_PKG_VERSION"));
        return;
    }
    if let Err(e) = sts2_modsync::gui::run() {
        eprintln!("sts2-modsync-gui: error fatal: {e}");
        std::process::exit(1);
    }
}
