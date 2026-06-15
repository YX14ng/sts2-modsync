# CLAUDE.md

Guia para Claude Code en este repo. **Antes de tocar codigo, lee [HANDOFF.md](HANDOFF.md)** —
tiene el research (transporte/costo, stack, deteccion, seguridad) ya hecho y el plan de fases.
El plan vivo hacia 1.0 (auditoria en 6 dimensiones, Definition of Done) esta en [ROADMAP.md](ROADMAP.md).

## Que es

`sts2-modsync` — **mod manager para Slay the Spire 2** (Rust/Windows): detecta el install
(Steam o copias pirata via dialogo), **lista / habilita / deshabilita / instala / desinstala**
mods, gestiona **perfiles** y el **orden de carga**, y **lanza** el juego. La **sincronizacion
de sets** entre un modder y sus amigos (gratis via GitHub Releases, rapida por hash) es **un
modulo mas** (pestaña Sync). GUI-first (eframe) + CLI.

## Estado

**v1.10.0 (estable).** Las fases 0.4-0.7 del [ROADMAP.md](ROADMAP.md) (integridad transaccional,
seguridad de la cadena, distribuible/diagnosticable, pulido UX) estan hechas y revisadas; el DoD
esta completo. Los tres features post-1.0 tambien estan hechos: single `.exe` (1.1.0), login de
GitHub + publish por API REST sin `gh` (1.2.0), firma `.minisig` opcional para sets (1.3.0). Mas:
- **1.4.0:** la app **recuerda el repo de publicacion** (`config.publish_repo`), asi "actualizar la
  lista" es subir OTRO release al MISMO repo (no recrear repos); el `--repo`/tag se sanean antes de
  armar el `base_url` firmado.
- **1.5.0:** podes **suscribirte a un REPO** (`repo:owner/repo` en `subscribed_sets`) que sigue el
  ULTIMO release: `transport::resolve_latest_manifest` consulta `/releases/latest` (sin login) y arma
  la URL del manifest. Las suscripciones por URL fija de antes siguen andando (sin migracion).
- **1.6.0:** **delta intra-`.pck`** (modulo `delta`, bsdiff via `qbsdiff`): al actualizar un mod, el
  amigo que ya tiene la version vieja baja **solo el diff**, no el `.pck` entero. `publish` genera los
  patches contra la publicacion anterior en `--out`; `sync` elige el patch si el archivo viejo local
  matchea un `delta.from_blake3` y verifica el BLAKE3 del resultado (si falla, cae al full). Seguro
  por construccion: el delta es pura optimizacion, nunca puede instalar bytes equivocados.
- **1.7.0 (fase 1):** **auto-update de MODS desde su upstream.** `modsource::ModSource` (GitHub o
  Nexus) sale del `<id>.json` (`repository`/`url`/`homepage`) o lo pega el usuario (`config.mod_sources`,
  prioridad). `modupdate::check_github` lista `/releases` y elige por canal GLOBAL (`config.prefer_beta`:
  beta = pre-releases, estable = MAIN) — el mapeo BETA/MAIN es limpio en GitHub. `modupdate::apply` baja
  el asset `.zip` e instala con `manager::install_update_zip` (valida el id == el mod), preservando
  enable/disable y recordando el tag (`config.mod_installed_tag`). `mod_dir` deshabilitado se respeta.
- **1.8.0 (fase 2a):** **Nexus.** Modulo `nexus`: API Key personal en el llavero (`store_key`/`load_key`),
  `validate()` (usuario/Premium) y `check(game, mod_id, current)` (version del mod via la API v1).
  `modupdate::check_nexus` lo envuelve en un `ModUpdate` con `asset_url` VACIO (sin auto-download).
- **1.9.0 (fase 2b):** **descarga auto de Nexus via `nxm://`.** Modulo `nxm`: `parse_nxm_link` +
  `register`/`unregister`/`is_registered` (handler del protocolo en HKCU via `winreg`, solo Windows).
  `nexus::download_link` resuelve el CDN (con `key`/`expires` del link, o directo Premium). CLI
  `nxm <link>` (lo invoca Windows al tocar "Mod Manager Download"): baja con `transport::download_capped`
  e instala el `.zip`; `.7z`/`.rar` se guardan a Descargas (no se extraen). Resultado en un dialogo (rfd).
  GUI: boton "Registrar Mod Manager Download (nxm://)".
- **1.10.0:** **elegir/crear el repo de publicacion** y **actualizar mods de Nexus DIRECTO (Premium)**.
  GitHub: `github::Api::list_repos` (pagina `/user/repos`, filtra por permiso de push) + `create_repo`
  (POST `/user/repos`, devuelve `owner/repo`); el GUI (pestaña Publicar, con login) muestra un combo
  para elegir y un campo para crear, y recuerda lo elegido al toque. Nexus Premium: `nexus::latest_main_file`
  resuelve el archivo MAIN; `modupdate::check_nexus(.., premium)` mete un `NexusRef` en el `ModUpdate`
  cuando la cuenta es Premium, y `modupdate::apply_nexus` baja por `download_link` directo (sin
  `key/expires`) e instala con `install_update_zip` (valida id). El GUI valida la key al abrir para
  saber si sos Premium y muestra "Actualizar (Premium)"; las cuentas gratis siguen con `nxm://`. CLI:
  `mod-update <id>` ya actualiza mods de Nexus si la cuenta es Premium.

Detalle por version en [CHANGELOG.md](CHANGELOG.md). Lo que sigue (sin empezar): soporte `.7z` para
los mods de Nexus, crear el repo de mods con un click, OAuth `OAUTH_CLIENT_ID` real, delta zstd, y
confirmar en el flujo `nxm` antes de reemplazar si el id del `.zip` colisiona con OTRO mod instalado
(hoy `install_from_zip(overwrite=true)` lo manda a la papelera sin preguntar; reversible, no rompe
invariantes, pero el flujo lanzado por el protocolo no tiene prompt).

- **Mod manager (hecho, compila):** lista/detalle, enable/disable (= mover carpeta), instalar
  (carpeta/.zip) / desinstalar (papelera), perfiles, lanzar el juego, deps/conflictos, orden de
  carga canonico. GUI con pestañas **Mods|Sync|Perfiles** + CLI (`list/enable/disable/launch/sync`).
- **Sync (añadido, funcional):** `plan()` (dry-run) + `apply()` TRANSACCIONAL real — baja de un
  GitHub Release (`reqwest` **blocking**, sin tokio), verifica BLAKE3, renombra, manda huerfanos a
  la papelera, aborta si el juego corre. La pestaña Sync del GUI baja/instala de verdad.
- **Publicar (añadido, modder):** `publish` genera el set-manifest + assets desde tus mods (hashea
  BLAKE3) y **los SUBE al GitHub Release** via `gh` CLI (`publish::upload` deriva owner/repo/tag del
  `--base-url` y sube manifest + `.minisig` + assets en lotes; `--no-upload` solo genera local). Sin
  esa subida el Release queda vacio y la sync por URL daba 404. Los assets son **content-addressed**
  (nombre = el blake3): los assets de un Release son PLANOS, asi que el transporte baja por
  `base_url + blake3`, NO por `entry.path` (que queda solo para instalar local).
  **FASE 3:** delta intra-`.pck` (bita), auto-update, HTTP Range/resume. Detalle en HANDOFF.md.
  **OJO publicar sets vs auto-update:** publica los SETS DE MODS en un repo APARTE del de la app. Si
  usas el mismo, el filtro `v*` del auto-update (`update::check_latest` lista `/releases` y elige el
  mayor tag `vX.Y.Z`) evita que un release de mods (tag tipo `2026.06.14`) dispare un update falso.

**Firma minisign (crate `minisign`) — DOS modelos (desde 1.3.0, ver `signing.rs`):**
`PUBLISHER_PUBKEY` tiene la clave publica del publicador. La clave SECRETA vive fuera del repo: en
`%APPDATA%/.../minisign.key` (la genero `keygen`) y como secret de GitHub Actions `MINISIGN_SECRET_KEY`.
- **set-manifests (sync): firma OPCIONAL** (`verify_optional`). El ancla de confianza es HTTPS + la
  URL del publicador (su repo de GitHub) + el content-addressing por BLAKE3. Si el set trae
  `set-manifest.json.minisig` se valida (capa extra) y una firma invalida se rechaza; si no, se
  acepta como `Unsigned` (la UI lo muestra: verde "verificada" / naranja "sin firma"). Un
  publicador NO necesita manejar una clave para compartir sets.
- **binario de auto-update: firma OBLIGATORIA** (`verify_with_embedded`, estricto). El CI corre
  `sts2-modsync sign <zip>` (`MINISIGN_SECRET_KEY`) y sube `<zip>.minisig`; `update::apply` lo baja
  y verifica ANTES de reemplazar el exe (cierra el vector "release malicioso"). CLI `sign <archivo>`.

`eframe` es dep **opcional** (feature `gui`); el resto del core (`reqwest`/`zip`/`trash`/`minisign`/
`self-replace`) es dep normal.

## Arquitectura (modulos en `src/`)

- **Core:** `detect` (Steam/pirata + juego-abierto) · `config` (%APPDATA%).
- **Mod manager:** `modlist` (escanea `mods/`+`mods_disabled/`, parsea `<id>.json`, deps/conflictos,
  orden de carga; `ModManifest.source_hint()` lee el upstream del mod) · `manager` (enable/disable/
  install/uninstall = **MOVER carpetas**, juego cerrado) · `profile` (perfiles = conjuntos habilitados)
  · `launch` (abrir el juego) · `modsource` (`ModSource` GitHub/Nexus: parse/storage/web_url) ·
  `modupdate` (auto-update de un mod desde su upstream: `check_github`/`check_nexus` + `apply`
  baja+instala) · `nexus` (API v1 de Nexus: API key en el llavero + `validate` + `check` + `download_link`)
  · `nxm` (handler del protocolo `nxm://`: parse + registro en HKCU, solo Windows).
- **Sync (añadido):** `manifest` (set-manifest + validacion paths + toposort; `FileEntry.deltas`)
  · `hashing` (blake3) · `sync` (`plan()` elige delta vs full + `apply()` transaccional con
  delta/fallback) · `delta` (bsdiff via `qbsdiff`: `diff()` lado publish, `apply()` lado sync; el
  resultado SIEMPRE se re-verifica por blake3, sino cae al full) · `signing` (minisign verify)
  · `transport` (GitHub Releases, `reqwest` blocking, **content-addressed por blake3** —fulls Y
  patches—; `resolve_latest_manifest` resuelve el ultimo release de un repo; el trait `ModSource`
  tiene un `prepare()` opcional que un backend usa para pre-bajar el set entero) · `publish` (genera
  el set-manifest + assets + **deltas** desde los mods y los SUBE al Release, lado modder).
- **P2P (añadido, feature `p2p`):** `torrent` (librqbit + tokio, gateado): `create_set_torrent`
  arma el `.torrent` del dir de assets y el magnet (lo mete `publish` en el manifest ANTES de
  firmar) · `seed_blocking` seedea el dir de assets (archivos ya presentes) · `HybridSource`
  implementa `ModSource`: `prepare` se une al swarm y baja los archivos pedidos a un staging,
  `fetch` los copia a destino, y si **no hay seeder** cae a `GitHubReleases` (HTTP). El magnet va
  en el manifest FIRMADO; `apply` igual verifica BLAKE3, asi que bajar de un peer es seguro. Envs
  avanzados (LAN/tests): `STS2_P2P_PEERS=ip:port,...`, `STS2_P2P_SEED_PORT`, `STS2_P2P_NODHT`.
- **Front:** `main` (CLI con subcomandos) · `gui/` (eframe, feature `gui` que INCLUYE `p2p`):
  partido en submodulos — `gui/mod.rs` (chasis: struct `App` con TODOS los campos privados, `new`,
  tema, `run`, topbar/nav, dispatcher `ui()`, y los metodos transversales scan/accion/toast/auto-update)
  · `widgets` (free fns de presentacion: `card`/`human_*`/toasts/onboarding) · `mods_tab` · `sync_tab`
  (estado `SyncState` + workers de fetch/plan/apply + suscripciones) · `publish_tab` (+ seed P2P)
  · `profiles_tab` · `github_login`. Cada tab aporta un `impl App` parcial; un submodulo HIJO ve los
  campos privados de `App` (definido en `mod.rs`), asi NO hay que volver `pub` el estado. Lo que el
  padre nombra de un hijo (tipos en campos de `App`, free fns compartidas) va `pub(super)`. `lib.rs`
  reexporta. **Para tocar una pestaña, editas SU archivo, no un monolito.**

Dos artefactos JSON distintos, **NO confundir**: el **`<id>.json`** que cada mod trae para el juego
(modelo en `modlist::ModManifest`) y el **set-manifest** de la sync (`manifest::SetManifest` /
`set-manifest.example.json`, describe un set entero a sincronizar).

## Comandos

- GUI (mod manager): `cargo run --features gui` SIN argumentos (single-exe: el mismo binario
  `sts2-modsync` abre la GUI si no hay subcomandos). Pestañas Mods/Sync/Perfiles/Publicar.
- CLI: `cargo run -- list` (default) · `enable/disable <id>` · `launch` · `sync <set.json|url|owner/repo>`
  (dry-run; con `owner/repo` —o `repo:owner/repo`— sigue el ULTIMO release via `/releases/latest`)
  · `mod-source <id> <usuario/repo|URL>` (fija el origen de un mod) · `mod-check [<id>]` (busca version
    nueva por canal global) · `mod-update <id>` (baja+instala la nueva, origen GitHub)
  · `publish --name <s> --version <v> [--repo <owner/repo> | --base-url <url>] [--profile <p>] [--out <dir>] [--no-upload] [--no-delta]`
    (modder; por default SUBE al Release. El **`--repo` se RECUERDA** en `config.publish_repo`: la
    proxima vez podes omitirlo y publica OTRO release en el MISMO repo —el GUI deriva el `base_url`
    del repo recordado—. `--base-url` sigue funcionando (legacy). `--no-upload` solo genera local.
    Genera **deltas** contra la publicacion anterior en `--out` salvo `--no-delta`)
  · `update` (auto-update desde GitHub Releases de `YX14ng/sts2-modsync`)
  · `keygen` (par minisign del modder; pegar la pub en `signing::PUBLISHER_PUBKEY` para activar firma)
  · `github-login <token>` / `github-status` / `github-logout` (token de GitHub guardado SEGURO en el
    llavero del SO via `keyring`; con login, `publish` sube por la **API REST** sin el `gh` CLI —
    modulo `github`: PAT o OAuth device-flow si se setea `github::OAUTH_CLIENT_ID`)
  · `nexus-login` / `nexus-status` / `nexus-logout` (API Key de Nexus guardada en el llavero; lee de
    `NEXUS_APIKEY` o stdin; habilita el chequeo de version de mods de Nexus — `mod-check`/GUI)
  · `nxm-register` / `nxm-unregister` (alta/baja del handler `nxm://` en HKCU) · `nxm <link>` (lo
    INVOCA Windows al tocar "Mod Manager Download" en Nexus: baja+instala; resultado en un dialogo)
  · `seed <out_dir>` (P2P: seedea un set publicado por torrent; bloquea hasta Ctrl-C; necesita
    `--features p2p`. En el GUI: boton "Seedear este set (P2P)" en la pestaña Publicar).
- `cargo test` · `cargo clippy --all-targets --features gui` · `cargo fmt` · `cargo build --release`.
- P2P (torrent): `cargo build --features p2p` (CLI con `seed`) o `--features gui` (ya incluye p2p).
  Test e2e real de P2P (loopback, abre sockets, ignorado por default):
  `cargo test --features p2p p2p_loopback -- --ignored --nocapture --test-threads=1`.
- Un solo test: `cargo test <nombre>` (o por modulo `cargo test modlist::tests::`); `-- --nocapture`
  para ver prints. Tests inline en casi todos los modulos —incluidos los **peligrosos**
  (`manager`/`update`/`transport`, red de seguridad de la fase 0.3)— ademas de
  `manifest`/`modlist`/`profile`/`sync`/`publish`/`signing`/`detect` (varios crean mods de prueba en
  un tempdir; `sync::apply` usa un `ModSource` falso, `publish` hace round-trip prepare→plan=noop).
  NO pegan a la red.
- Agregar deps: `cargo add <crate>` (NO hardcodear patch a ojo — deja que cargo resuelva).
- Toolchain **MSVC** + VS Build Tools (sin OpenSSL; todo rustls — librqbit usa native-tls=SChannel
  en Windows, NO OpenSSL). El core ya incluye `zip`/`trash` (manager); `eframe` es opcional (feature
  `gui`); `librqbit`+`tokio` son opcionales (feature `p2p`, que `gui` incluye) y engordan el binario
  (por eso van gateados). Release size-optimized (`opt-level="z"`, `lto`, `panic="abort"`).

## CI / release (`.github/workflows/`)

- **`ci.yml`** (push a `main` + cada PR, windows-latest): `cargo fmt --all --check` ·
  `cargo clippy --all-targets --features gui -- -D warnings` (**un warning ROMPE el build**) ·
  `cargo test --features p2p` (el loopback P2P es `#[ignore]`, no corre) · `cargo check` (core/CLI
  sin features, que el build liviano siga verde) · build GUI+CLI. Corré ese mismo set local antes
  de pushear.
- **`release.yml`** (push de un tag `v*`): corre el MISMO gate fmt/clippy/test, buildea release,
  **firma** el `.zip` con `sign` (secret `MINISIGN_SECRET_KEY`, si esta) y crea el GitHub Release
  (zip + `.minisig`). Lo consume el auto-update. Sacar version: subir `version` en `Cargo.toml`,
  `git tag vX.Y.Z && git push origin vX.Y.Z`.

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
