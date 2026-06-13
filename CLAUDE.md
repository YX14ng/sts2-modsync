# CLAUDE.md

Guia para Claude Code en este repo. **Antes de tocar codigo, lee [HANDOFF.md](HANDOFF.md)** —
tiene el research (transporte/costo, stack, deteccion, seguridad) ya hecho y el plan de fases.

## Que es

`sts2-modsync` — **mod manager para Slay the Spire 2** (Rust/Windows): detecta el install
(Steam o copias pirata via dialogo), **lista / habilita / deshabilita / instala / desinstala**
mods, gestiona **perfiles** y el **orden de carga**, y **lanza** el juego. La **sincronizacion
de sets** entre un modder y sus amigos (gratis via GitHub Releases, rapida por hash) es **un
modulo mas** (pestaña Sync). GUI-first (eframe) + CLI.

## Estado

- **Mod manager (hecho, compila):** lista/detalle, enable/disable (= mover carpeta), instalar
  (carpeta/.zip) / desinstalar (papelera), perfiles, lanzar el juego, deps/conflictos, orden de
  carga canonico. GUI con pestañas **Mods|Sync|Perfiles** + CLI (`list/enable/disable/launch/sync`).
- **Sync (añadido, funcional):** `plan()` (dry-run) + `apply()` TRANSACCIONAL real — baja de un
  GitHub Release (`reqwest` **blocking**, sin tokio), verifica BLAKE3, renombra, manda huerfanos a
  la papelera, aborta si el juego corre. La pestaña Sync del GUI baja/instala de verdad.
- **Publicar (añadido, modder):** `publish` genera el set-manifest + assets desde tus mods (hashea
  BLAKE3) y arma todo para subir a un GitHub Release (CLI + pestaña Publicar). Los assets son
  **content-addressed** (nombre = el blake3): los assets de un Release son PLANOS, asi que el
  transporte baja por `base_url + blake3`, NO por `entry.path` (que queda solo para instalar local).
  **FASE 3:** delta intra-`.pck` (bita), auto-update, HTTP Range/resume. Detalle en HANDOFF.md.

`signing::PUBLISHER_PUBKEY` vacia = **modo dev** (firma NO verificada — setear la clave real antes
de un release publico). `eframe` es dep **opcional** (feature `gui`); el resto del core (incl.
`reqwest`/`zip`/`trash`) es dep normal. La descarga corre en el worker thread del GUI (por eso
blocking, no async).

## Arquitectura (modulos en `src/`)

- **Core:** `detect` (Steam/pirata + juego-abierto) · `config` (%APPDATA%).
- **Mod manager:** `modlist` (escanea `mods/`+`mods_disabled/`, parsea `<id>.json`, deps/conflictos,
  orden de carga) · `manager` (enable/disable/install/uninstall = **MOVER carpetas**, juego cerrado)
  · `profile` (perfiles = conjuntos habilitados; puente con set-manifest) · `launch` (abrir el juego).
- **Sync (añadido):** `manifest` (set-manifest + validacion paths + toposort) · `hashing` (blake3)
  · `sync` (`plan()` + `apply()` transaccional) · `signing` (minisign verify) · `transport` (GitHub
  Releases, `reqwest` blocking, **content-addressed por blake3**) · `publish` (genera el
  set-manifest + assets desde los mods, lado modder).
- **Front:** `main` (CLI con subcomandos) · `gui` (eframe, pestañas; feature `gui`). `lib.rs` reexporta.

Dos artefactos JSON distintos, **NO confundir**: el **`<id>.json`** que cada mod trae para el juego
(modelo en `modlist::ModManifest`) y el **set-manifest** de la sync (`manifest::SetManifest` /
`set-manifest.example.json`, describe un set entero a sincronizar).

## Comandos

- GUI (mod manager): `cargo run --features gui --bin sts2-modsync-gui` (pestañas Mods/Sync/Perfiles/Publicar).
- CLI: `cargo run -- list` (default) · `enable/disable <id>` · `launch` · `sync <set.json>` (dry-run)
  · `publish --name <s> --version <v> --base-url <url> [--profile <p>] [--out <dir>]` (modder)
  · `update` (auto-update desde GitHub Releases de `YX14ng/sts2-modsync`).
- `cargo test` · `cargo clippy --all-targets --features gui` · `cargo fmt` · `cargo build --release`.
- Un solo test: `cargo test <nombre>` (o por modulo `cargo test modlist::tests::`); `-- --nocapture`
  para ver prints. Tests inline en `manifest`/`modlist`/`profile`/`sync`/`publish` (varios crean
  mods de prueba en un tempdir; `sync::apply` usa un `ModSource` falso, `publish` hace round-trip
  prepare→plan=noop). NO pegan a la red.
- Agregar deps: `cargo add <crate>` (NO hardcodear patch a ojo — deja que cargo resuelva).
- Toolchain **MSVC** + VS Build Tools (sin OpenSSL; todo rustls). El core ya incluye `zip`/`trash`
  (manager); `eframe` es opcional (feature `gui`). Release size-optimized (`opt-level="z"`, `lto`,
  `panic="abort"`).

## Invariantes que NO romper

- **Seguridad (baja DLLs que el juego ejecuta):** firma del manifiesto (P0) + hash por
  archivo + HTTPS. Ver §seguridad de HANDOFF.
- **Nunca tocar carpetas fuera de `manifest.managed_ids()`** (no pisar mods ajenos del amigo).
- **Path-traversal:** `manifest::validate_paths` y `manager::safe_id` rechazan `..`/sep/absolutas.
- **Manager = mover carpetas, juego cerrado:** enable/disable mueven `mods/<id>` ↔
  `mods_disabled/<id>` (hermano que el juego NO escanea); install copia, uninstall manda a la
  papelera. Toda mutacion exige `detect::is_game_running()==false`. NO se toca `setting.save`.
- **Orden de carga (multiplayer):** el room-hash de BaseLib depende del ORDEN de carga; si difiere
  entre amigos no entran al lobby. El set DEBE incluir **BaseLib + ModListSorter** (el enforcer que
  fija BaseLib+A-Z en runtime al cerrar el juego). El programa deriva/muestra ese orden con
  `manifest::canonical_load_order` (distinto del toposort `install_order`) y advierte si falta
  ModListSorter. NO se toca `setting.save` (save de Godot, fragil) — ModListSorter es la autoridad.
- **Apply transaccional:** todo a `.part` + verificado, luego renames atomicos juntos; abortar
  si el juego corre (lock de `.dll/.pck` en Windows).

## Convenciones

- Config local: `config.local.toml` / `%APPDATA%/sts2-modsync/config.toml` (gitignorado).
  Plantilla no-secreta: `config.example.toml`.
- No versionar blobs (`.pck`, `.dll`, `*.pdb`) ni `/test-mods` (gitignorado).
- El autor escribe en **espanol, sin tildes ni diacriticos** (ASCII: `deteccion`, no
  `detección`); igualar el idioma y ese estilo al editar comentarios/docs.
