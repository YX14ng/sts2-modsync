//! Build script: embebe el icono de la app como icono del `.exe` en Windows (lo que muestra el
//! Explorador / la barra de tareas / un acceso directo). El icono de la VENTANA lo pone eframe con la
//! MISMA generacion (`crate::icon`). Best-effort: si falta el compilador de recursos, AVISA y sigue
//! (el exe se arma igual, solo sin icono en el Explorador) — el icono nunca debe romper el build/CI.

// Reusa la misma generacion del icono que la ventana (solo usa `std`, por eso se puede `include!`).
include!("src/icon.rs");

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    // El icono del exe es cosa de Windows: solo cuando el TARGET es Windows.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if let Err(e) = embed_exe_icon() {
        println!(
            "cargo:warning=no se pudo embeber el icono del exe (se compila igual sin el): {e}"
        );
    }
}

fn embed_exe_icon() -> std::io::Result<()> {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR lo setea cargo");
    let ico_path = std::path::Path::new(&out_dir).join("app-icon.ico");
    // .ico multi-resolucion (16..256) para que se vea nitido en todos los tamaños del Explorador.
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16u32, 32, 48, 64, 128, 256] {
        let img = ico::IconImage::from_rgba_data(size, size, rgba(size));
        dir.add_entry(ico::IconDirEntry::encode(&img)?);
    }
    dir.write(std::fs::File::create(&ico_path)?)?;

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico_path.to_str().expect("ruta del .ico no-UTF8"));
    res.compile()
}
