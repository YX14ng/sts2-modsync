# HANDOFF — sts2-modsync

Para el proximo Claude Code que continue este programa. Lee esto PRIMERO. El research
(transporte/costo, stack Rust, deteccion, seguridad) ya esta hecho y verificado — abajo
esta destilado para que NO tengas que re-investigar.

## Que es

App de escritorio en **Rust para Windows**. Empezo como sincronizador de sets y ahora es un
**mod manager completo de StS2** (lista/enable/disable/instalar/desinstalar/perfiles/lanzar +
orden de carga), donde la **sync es un modulo mas**. Sigue (1) **detectando** el install
— o dejando **elegir la carpeta** (varios la tienen PIRATA, fuera de Steam) — y (2)
**sincronizando sets de mods** (un modder publica; sus amigos bajan/actualizan), **rapido y
barato**. La arquitectura actual (modulos `modlist`/`manager`/`profile`/`launch` + el GUI con
pestañas) esta en **CLAUDE.md**; abajo queda el research de la sync (transporte/seguridad/fases),
que sigue vigente para FASE 2.

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
- **Orden de carga (multiplayer) != toposort.** BaseLib calcula un *room-hash* con los mods
  cargados + su ORDEN; si difiere entre amigos, el juego los bloquea del lobby del otro. El orden
  canonico es **BaseLib primero, el resto A-Z** (lo fuerza el mod **`ModListSorter`** al cerrar el
  juego). El set DEBE incluir `ModListSorter`; el cliente lo deriva con
  `SetManifest::canonical_load_order()` y advierte (CLI/GUI) si falta (`syncs_load_order()`). El
  orden + enabled viven en `setting.save` (save de Godot) — **NO lo tocamos** (fragil, atado a la
  version; habria que reversear el formato): confiamos en ModListSorter como enforcer en cada
  maquina. `ModSyncChecker` es la contraparte in-game (compara lista/orden/hash con el host).

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

Deps ya agregadas: `reqwest` 0.12 (**blocking** + rustls), `eframe` (feature `gui`), `zip`/`trash`
(manager). Se uso reqwest **BLOCKING** (no async): la descarga corre en el worker thread del GUI,
asi que **no hizo falta tokio/futures-util**. Tampoco `tempfile`: los `.part` van en la carpeta
destino (mismo volumen). FASE 3: `zstd`/`bita` siguen comentadas.
⚠️ `cargo add` para resolver versiones (NO hardcodear): reqwest 0.13 cambio el nombre del feature
TLS, por eso se fijo 0.12 (tiene `rustls-tls`+`blocking`); egui 0.34.3 resulto real.

1. **Transporte** ✅ HECHO (`transport.rs`): `GitHubReleases::fetch` con `reqwest::blocking`+rustls
   (sin OpenSSL en MSVC), baja por `browser_download_url` directo con callback de progreso por chunk
   + chequeo de tamaño. (Sin HTTP Range/resume aun: el `.part` se rehace; resume = mejora futura.)
2. **apply()** ✅ HECHO (`sync.rs`): transaccion all-or-nothing — baja TODO a `.part` + verifica
   BLAKE3, recien entonces renombra (orden topologico); huerfanos a la papelera (`trash`); aborta si
   el juego corre. Tests con `ModSource` falso (`GoodSource`/`BadSource`, sin red).
3. **GUI** ✅ HECHO (`src/gui.rs`, feature `gui`, bin `sts2-modsync-gui`): 3 pantallas
   (detectar/elegir -> revisar+consentir -> progreso) immediate-mode, con worker en hilo +
   canal `mpsc` + `ctx.request_repaint()` ya cableado (el hashing del plan corre off-UI). La
   pantalla 3 llama a `apply()` (stub) — se vuelve real al terminar #1-#2. OJO: eframe 0.34
   usa `App::ui(&mut self, ui, frame)` (no `update`); paneles via `show_inside`. Reusa el core.
4. **Modo PUBLICAR** ✅ HECHO (`src/publish.rs`, CLI `publish` + pestaña Publicar): `prepare()`
   hashea los mods elegidos -> set-manifest + assets; `write_out()` escribe
   `out/set-manifest.json` + `out/assets/<blake3>`; `gh_hint()` imprime el `gh release create`.
   **OJO esquema CONTENT-ADDRESSED:** los assets de un Release son PLANOS y GitHub sanitiza
   nombres, asi que NO se puede subir "BaseLib/BaseLib.dll". El asset se llama por su **blake3**
   (`transport` baja por `base_url + entry.blake3`; `entry.path` queda solo para instalar local).
   Da dedup gratis. **Firma:** v1 va SIN firmar (dev mode alcanza entre amigos); integrar
   sign/keygen (crate `minisign`) = follow-up. Subir auto via `gh` desde la app = follow-up.

## FASE 3 — opcional
- Delta intra-archivo del `.pck` con `bita` (verificar el supuesto de Godot primero).
- **Auto-update ✅ HECHO** (`src/update.rs`): chequea el ultimo release de `YX14ng/sts2-modsync`,
  compara con `CARGO_PKG_VERSION`, baja el `.zip`, extrae `sts2-modsync-gui.exe`, `self-replace`
  (reemplaza el exe en uso) y relanza. GUI = banner "Actualizar ahora"; CLI = `update`. El repo es
  PUBLICO; el CI (`.github/workflows/release.yml`) publica el zip al pushear un tag `vX.Y.Z` (eso es
  lo que come el auto-update). Seguridad: baja/ejecuta un binario del PROPIO release por HTTPS
  (ancla = dueño del repo). HTTP Range/resume del transporte sigue pendiente.
- Mirror Cloudflare R2.

## Stack elegido (verdicts del research)
GUI **egui+eframe** (single-exe, sin WebView; Tauri descartado por WebView2/JS; iced/slint
viables). HTTP **reqwest+rustls**. Dialogo **rfd**. Hash **blake3**. Deteccion **steamlocate**.
Config **serde+toml**. Todo MIT/Apache, mantenido, compila en `x86_64-pc-windows-msvc`.

## Build
`cargo run` (dev) · `cargo run -- <manifiesto.json>` (dry-run del plan) · `cargo test` ·
`cargo build --release` (perfil opt-level=z+lto, ~8-15 MB). Toolchain: Rust + VS Build Tools (MSVC).
