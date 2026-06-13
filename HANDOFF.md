# HANDOFF — sts2-modsync

Para el proximo Claude Code que continue este programa. Lee esto PRIMERO. El research
(transporte/costo, stack Rust, deteccion, seguridad) ya esta hecho y verificado — abajo
esta destilado para que NO tengas que re-investigar.

## Que es

App de escritorio en **Rust para Windows** que (1) **detecta** donde esta instalado Slay
the Spire 2 — o deja al usuario **elegir la carpeta** (porque varios la tienen PIRATA,
fuera de Steam) — y (2) **sincroniza sets de mods** (un modder publica; sus amigos bajan/
actualizan). Requisito del usuario: que sea **rapido y barato**.

## Estado actual — FASE 1 (HECHA, compila: `cargo check` verde)

Core agnostico de GUI/red, ya funcional:
- `src/detect.rs` — cascada Steam (`steamlocate`, AppID **2868840**) -> rutas comunes ->
  dialogo `rfd` (pirata). Valida por heuristica (`SlayTheSpire2.exe` + `data_sts2_windows_x86_64/`).
  Detecta juego abierto (`sysinfo`).
- `src/manifest.rs` — modelo del **set-manifest** (contrato central) + validacion de
  paths (anti traversal), toposort de dependencias.
- `src/hashing.rs` — BLAKE3 por archivo (mmap+rayon) = capa delta gruesa.
- `src/sync.rs` — `plan()` calcula que bajar / que esta al dia / huerfanos (acotados a
  carpetas gestionadas) + orden topologico. `apply()` = **stub FASE 2** (contrato escrito).
- `src/signing.rs` — verificacion de firma `minisign` del manifiesto (clave publica
  empotrada). P0 de seguridad.
- `src/transport.rs` — **trait** `ModSource` + stub `GitHubReleases` (sin reqwest aun).
- `src/config.rs` — config local en `%APPDATA%/sts2-modsync/config.toml`.
- `src/main.rs` — **MVP CLI**: detecta + lee un manifiesto local + imprime el plan (dry-run).

Probalo: `cargo run -- set-manifest.example.json` (detecta tu StS2 y muestra el plan).

## Decision de metodo (rapido + barato) — del research, NO la cambies sin razon

**Transporte: GitHub Releases.** El modder ya vive en GitHub; los amigos bajan sin cuenta
(URL anonima); gratis; sin limite de banda ni de tamano total; cada `.pck` de 100+ MB entra
bajo el tope de **2 GiB por asset**. Cada "set" = un release con tag. Bajar por la
`browser_download_url` **directa**, NO via `api.github.com` (rate-limit anonimo 60 req/h
desde 2025-05-08; las descargas de assets NO cuentan, pero listar releases por API si).
Respaldo opcional: **Cloudflare R2** (10 GB gratis, egress $0) como mirror/CDN.

**Delta en 2 capas:**
1. **Gruesa (obligatoria, ya implementada)** — manifiesto de hashes BLAKE3 por archivo;
   el cliente baja SOLO los archivos cuyo hash difiere. Como normalmente cambia 1 personaje,
   esto ya da ~80% del ahorro (no rebaja BaseLib/FGOCore/otros personajes).
2. **Fina (FASE 3, opcional)** — delta intra-archivo del `.pck` con **`bita`** (chunking por
   contenido tipo FastCDC + Range): aunque el `.pck` se regenera entero, Godot empaqueta en
   orden estable y la mayoria de los chunks se reutilizan. ⚠️ VERIFICAR empiricamente que
   tocar 1 recurso no reescribe todo el `.pck` (regenerar tras un cambio minimo y medir % de
   chunks reutilizables); si MegaDot reordena todo, quedarse con la capa gruesa. `bita` tiene
   pocos usuarios/1 mantenedor → NO en el camino critico.

Costo estimado: **$0** (GitHub Releases publico).

## Formato del set-manifest (ver `set-manifest.example.json`)

Artefacto NUEVO (no confundir con el `<Id>.json` del juego). Campos en `manifest.rs`.
Reglas de seguridad codificadas en `SetManifest::validate`:
- `mods[].id` = el conjunto EXACTO de carpetas que el sync puede tocar (`mods/<id>/`).
  Cualquier carpeta no listada es **intocable** (asi no se pisan los mods ajenos del amigo).
- `files[].path` relativa a `mods/`, debe quedar dentro de un `<id>/` listado, sin `..`,
  sin raiz absoluta (cierra path-traversal que escaparia a sobreescribir archivos del juego).
- `dependencies` -> `install_order()` topologico (BaseLib -> FGOCore -> personajes). El set
  DEBE incluir las librerias compartidas como entradas propias.

## Modelo de seguridad (esto baja DLLs que el juego EJECUTA — no es opcional)

Prioridad:
- **P0 — firma + hash.** `minisign`/ed25519 del manifiesto, clave publica fijada en el cliente
  (`signing::PUBLISHER_PUBKEY`, hoy vacia = modo dev). Hash BLAKE3 por archivo verificado tras
  bajar. Cierra "atacante sustituye el .dll en el hosting / MITM". (Autenticidad ≠ inocuidad:
  el amigo igual confia en el publicador; es para amigos.)
- **P1 — HTTPS** siempre.
- **P2 — consentimiento:** mostrar SIEMPRE la lista de mods + versiones + que se va a
  borrar/instalar ANTES de aplicar.
- **P3 — clave privada** fuera del repo; permitir rotacion via `signing_key_id`.

Riesgos verificados (mitigar al implementar `apply`):
- **Borrado destructivo:** un bug en "huerfanos" que mire fuera de `managed_ids` borraria mods
  ajenos. Acotado en `sync::plan`; al aplicar: backup `.bak`/papelera + dry-run que muestra que
  se borraria. NO ampliar el barrido fuera de `managed_ids`.
- **Lock de archivos (Windows):** con el juego abierto, `.dll/.pck` estan mapeados; escribir a
  media transaccion deja un set inconsistente (justo el `MissingMethodException` de FGOCore).
  → exigir juego cerrado (`detect::is_game_running()`), y transaccion **all-or-nothing**: bajar
  + verificar TODO a `.part`, recien entonces renombrar junto.
- **Version de BaseLib pinneada:** si el set trae personajes para otra BaseLib que la instalada
  → `ReflectionTypeLoadException`. Verificar `manifest.baselib_version` y advertir.
- **GitHub rate-limit (60 req/h anon):** leer el manifiesto de una URL raw fija / asset
  "latest" y bajar por `browser_download_url`; backoff. Un PAT read-only sube a 5000 req/h.
- **Path traversal:** ya rechazado en `validate_paths` (`..`, absolutas, `:`); mantenerlo.
- **Custodia de la clave privada:** si se filtra, cualquiera firma sets maliciosos → rotacion.

## FASE 2 — lo que sigue (en orden)

Descomenta en `Cargo.toml` las deps de FASE 2 (versiones/features ya elegidas):
```
reqwest      = { version = "0.12", default-features = false, features = ["rustls-tls", "stream", "http2"] }
tokio        = { version = "1", features = ["rt-multi-thread", "macros", "fs", "io-util"] }
futures-util = "0.3"
tempfile     = "3"
zstd         = "0.13"
eframe       = "0.31"   # GUI; forzar backend glow en NativeOptions (drivers viejos de amigos)
```
⚠️ El research alucino algunos patch (dijo reqwest 0.13.4 / egui 0.34.3 / tokio 1.52 — NO
existen): usa `cargo add` para resolver la version real (asi se hizo con el core).

1. **Transporte** (`transport.rs`): implementar `GitHubReleases::fetch` con reqwest+rustls
   (sin OpenSSL en MSVC), `bytes_stream()` para progreso, HTTP Range para reanudar sobre `.part`,
   verificar BLAKE3. Descargas concurrentes con `futures_util::StreamExt::buffer_unordered(N)`.
2. **apply()** (`sync.rs`): transaccion all-or-nothing (todo a `.part` + verificado, luego
   renames atomicos con `tempfile` en el MISMO volumen — cross-device falla en Windows),
   borrado de huerfanos con backup, en orden topologico, abortando si el juego corre.
3. **GUI** (`eframe`/egui, single-exe): 3 pantallas (detectar/elegir carpeta -> revisar set y
   confirmar -> progreso). egui es immediate-mode: descargas/hashing en hilo tokio aparte,
   progreso por canal + `ctx.request_repaint()`. NO bloquear el loop. Reusa todo el core.
4. **Modo PUBLICAR** (modder): generar el set-manifest (hashear `mods/<ids>/`), firmar con
   minisign, y subir a un GitHub Release (`gh release upload` o `octocrab`).

## FASE 3 — opcional
- Delta intra-archivo del `.pck` con `bita` (verificar el supuesto de Godot primero).
- Auto-update del propio `.exe` (`self_update` desde GitHub Releases).
- Mirror Cloudflare R2.

## Stack elegido (verdicts del research)
GUI **egui+eframe** (single-exe, sin WebView; Tauri descartado por WebView2/JS; iced/slint
viables). HTTP **reqwest+rustls**. Dialogo **rfd**. Hash **blake3**. Deteccion **steamlocate**.
Config **serde+toml**. Todo MIT/Apache, mantenido, compila en `x86_64-pc-windows-msvc`.

## Build
`cargo run` (dev) · `cargo run -- <manifiesto.json>` (dry-run del plan) · `cargo test` ·
`cargo build --release` (perfil opt-level=z+lto, ~8-15 MB). Toolchain: Rust + VS Build Tools (MSVC).
