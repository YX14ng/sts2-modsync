# Roadmap a 1.0.0 — sts2-modsync

> Plan derivado de una auditoria del codigo en 6 dimensiones (features/UX, robustez, testing,
> seguridad, distribucion, performance). Convencion del repo: espanol ASCII sin tildes.

## Orden de ejecucion (acordado)

1. **Quick wins / Fase 0.3** (red de seguridad) — se empieza por aca.
2. **Todo el ROADMAP** (fases 0.4 -> 1.0).
3. **Features post-1.0** (ver seccion final): single `.exe`, sacar dependencia del `.minisig`,
   login de GitHub en la app + crear repo publico de mods automatico.

## 1. Donde estamos

**1.0.0 — ALCANZADO.** Todos los criterios del Definition of Done (§2) estan cumplidos y las fases
0.3 → 0.7 (CI/tests, integridad transaccional, seguridad de la cadena, distribuible/diagnosticable,
pulido UX) estan completas, cada una revisada adversarialmente antes de su release. El flujo central
(detectar, mod manager, sync transaccional FIRMADA, publish, auto-update RECUPERABLE, P2P) es robusto,
seguro y comodo para no-tecnicos.

Lo que sigue son los **features post-1.0** (§7): single `.exe`, login de GitHub + repo de mods
automatico, y sacar la dependencia del `.minisig`. Fuera de 1.0 tambien: delta intra-`.pck`.

<details><summary>Contexto historico (estado en 0.2.3, antes de cerrar 1.0)</summary>

0.2.3 era funcionalmente completo pero NO en grado 1.0: faltaba **confianza/estabilidad/UX**.
Agujeros sistemicos que se cerraron: **(a)** no habia CI de test/lint (cerrado en 0.3); **(b)** los
modulos peligrosos (`manager.rs`, `transport.rs`, el `apply` del auto-update) tenian cero tests
(cerrado en 0.3/0.4/1.0); **(c)** faltaba LICENSE (cerrado en 0.3). Mas dos fallas de integridad
(rename no atomico en `sync::apply`, `is_game_running` fragil) cerradas en 0.4.
</details>

## 2. Criterios de 1.0.0 (Definition of Done)

> **TODOS cumplidos en v1.0.0** (verificado contra el codigo, no solo tildado).

- [x] CI en push/PR: `fmt --check` + `clippy -D warnings` + `cargo test` + `build --features gui`,
      y el mismo gate ANTES de `gh release create`. **(0.3)**
- [x] `manager.rs` con tests (enable/disable, uninstall, `install_from_zip`, `safe_id`, zip-slip). **(0.3)**
- [x] Auto-update con tests (`extract_named`, `release_from_json`, filtro de tags `v*`). **(0.3)**
- [x] `transport.rs` con tests (mock loopback: Range 206/200, tamano final, `join_url`). **(1.0.0)**
- [x] `sync::apply` realmente transaccional (rename con backup+rollback). **(0.2.4)**
- [x] `is_game_running` robusto (nunca mutar `mods/` con el juego abierto). **(0.2.4)**
- [x] Errores nunca tragados (huerfanos no borrados se reportan; hash-mismatch reintenta). **(0.2.4)**
- [x] Seguridad enforced en codigo (`http://` rechazado; zip-slip del install local cerrado). **(0.2.4)**
- [x] LICENSE + `license=` en Cargo.toml + README de usuario final (aviso SmartScreen). **(0.2.6)**
- [x] Auto-update recuperable (`.bak` del exe viejo + verificar arranque). **(0.2.6)**
- [x] Logging a archivo en %APPDATA% + panic-hook (el GUI no tiene consola). **(0.2.6)**
- [x] Config versionada (no perder `install_root`/`subscribed_sets` en silencio). **(0.2.6)**
- [x] Cancelacion + progreso detallado en sync/install. **(0.2.7)** (la de publish queda pendiente:
      el hasheo+subida es un one-shot menos critico de cortar.)
- [x] Feedback de UI honesto (`install_note` se renderiza; firma visible/afirmativa). **(0.2.6/0.2.7)**

## 3. Roadmap por fases (riesgo y dependencias primero)

### 0.3 — Red de seguridad (CI + tests de modulos peligrosos) · effort medio
- `ci.yml` en push/PR: fmt + clippy `-D warnings` + test + build gui. **(bloqueante, bajo)**
- Mismo gate en `release.yml` antes de `gh release create`.
- `tempfile` dev-dep -> temp-dirs hermeticos (sync/modlist/publish/torrent tests).
- Tests `manager.rs` (enable/disable, uninstall, `install_from_zip`, `safe_id`, zip-slip). **(bloqueante)**
- Tests auto-update (`extract_named`, `release_from_json`, filtro `v*`). **(bloqueante)**
- Tests `transport.rs` con mock loopback; correr el loopback P2P (hoy `#[ignore]`) en un job.
- `rust-toolchain.toml` (builds reproducibles).

### 0.4 — Integridad transaccional · effort medio · **HECHA (0.2.4)**
- [x] Rename transaccional con backup+rollback ante fallo parcial (backups en subdir reservado
  `mods/.modsync-backup/bak-<n>`, nombre libre para no pisar respaldos de un run previo). **(BLOQUEANTE)**
- [x] Endurecer `is_game_running` (decisor puro `any_is_game`: matchea nombre o basename del exe).
- [x] No tragar errores: `ApplyReport.orphans_failed` se reporta en la UI; `fetch_verified` reintenta.
- [x] Gestion de `.part`: excluidos de huerfanos (`is_part_file`) + `sweep_parts` barre los stale.
  (Limpiar el staging P2P de `HybridSource` queda pendiente — es interno de `torrent.rs`.)
- [x] Casos borde Windows: `long_path` (`\\?\` + UNC `\\?\UNC\`), zip-slip del install local
  (extraccion por `enclosed_name` + chequeo por componentes), pre-check de disco (`free_space_for`).
- [x] Resume Range que re-baja de cero si el `.part` quedo corrupto (truncado en el reintento).
- Validacion del `id` del manifest (cierra el escape de `mods_dir.join(id)` en orphan-scan/sweep).

### 0.5 — Seguridad de la cadena · effort medio · **HECHA (0.2.5)**
- [x] HTTPS enforced (`transport::require_https` rechaza `http://` en manifest, firma, assets y
  el zip+`.minisig` del auto-update). **(importante, bajo)**
- [x] Zip-slip del install local desde `.zip` cerrado (en 0.2.4: `enclosed_name` + componentes).
- [x] `cargo-audit` en CI (job en ubuntu; falla solo ante CVEs, no ante "unmaintained").
- [x] Verificacion de firma VISIBLE y afirmativa: `signing::SigStatus` -> verde "Firma verificada"
  / naranja "modo dev" en el GUI, y linea en la CLI (`sync`).
- [x] `SECURITY.md` (modelo de confianza + reporte) + tests negativos (`require_https`,
  `verify_with_embedded` exige firma cuando hay pubkey).

### 0.6 — Distribuible y diagnosticable · effort bajo-medio · **HECHA (0.2.6)**
- [x] LICENSE (MIT) + campo `license`. **(BLOQUEANTE legal, bajo)**
- [x] README usuario final (link al release, single-exe, aviso SmartScreen). **(BLOQUEANTE, bajo)**
- [x] Auto-update recuperable: `.bak` del exe viejo + `--health-check` del nuevo + rollback (y si
  el rollback falla, preserva el `.bak`).
- [x] Logging a `%APPDATA%/sts2-modsync/sts2-modsync.log` + panic-hook (con backtrace; rota a 1 MiB).
- [x] Config versionada (`schema`): config corrupta se respalda en `.toml.bad`, no se resetea en silencio.
- [x] CHANGELOG.md + `rel.notes` mostradas antes de actualizar (GUI colapsable + CLI).

### 0.7 -> 1.0 — Pulido de producto (UX) · effort medio-alto · **HECHA (0.2.7)**
- [x] Cancelacion de sync/install + limpieza al cancelar (la de publish queda pendiente).
- [x] Progreso detallado (archivo actual + velocidad/ETA) + throttle del repaint.
- [x] Arreglar `install_note` (ya se renderizaba) + onboarding (BaseLib/ModListSorter/orden de carga).
- [x] Toasts con auto-dismiss (exitos) + errores accionables (hint + boton cerrar).
- [x] Lista de Mods con orden ("habilitados primero") / filtro + boton "habilitar deps ya instaladas".
- [x] Sets suscritos con nombre legible + indicador "version nueva disponible" (chequeo manual).
- [x] Cache de hashes (path -> size+mtime+blake3) para no re-hashear GB.

## 4. Top 5 a atacar YA
1. `ci.yml` + gate en release.yml — la red de seguridad.
2. LICENSE + campo `license` — hoy nadie puede redistribuir el .exe.
3. Arreglar `install_note` — feedback perdido al elegir carpeta equivocada.
4. Endurecer `is_game_running` — un falso negativo corrompe el set con el juego abierto.
5. HTTPS enforced — alinea el codigo con el invariante de seguridad declarado.

## 5. Riesgos / decisiones del dueno
- **Code-signing Authenticode (pago)**: sin esto SmartScreen marca "editor desconocido". Documentar
  para 1.0, evaluar pagar despues.
- **Rotacion de clave minisign**: hoy una sola pubkey hardcodeada; documentar procedimiento.
- **Modelo de confianza (TOFU)**: pubkey global unica -> no escala a "mi amigo tambien publica".
- **Peso del binario con P2P**: GUI ~9.5 MB con librqbit+tokio activos aun para HTTP-only.
- **Soporte pirata** (documentar o implicito) y **telemetria/crash opt-in**.

## 6. Fuera de alcance de 1.0
Delta intra-`.pck` (bita/zstd), descargas concurrentes, evitar la copia staging->.part del P2P,
i18n (ingles/chino), pestana de Settings dedicada, fuzzing de `validate_paths`/`safe_id`, defensa
de downgrade en sync.

---

## 7. Post-1.0 (features pedidos, DESPUES de cerrar 1.0)

En este orden, una vez completado todo lo anterior:

1. **Un solo `.exe` para ejecutar.** ✅ **HECHO (1.1.0).** Hay UN binario `sts2-modsync.exe`: sin
   argumentos abre la GUI (doble-clic), con subcomandos es la CLI. Subsistema `windows` (sin consola
   negra al abrir el GUI) + `AttachConsole` para que el modo CLI muestre salida desde una terminal.
   (Sigue dependiendo del `gh` CLI para `publish` — eso lo cubre el feature 3.)

2. **Sacar la dependencia del `.minisig`.**
   > **OJO (decision de seguridad):** la firma minisign es el invariante P0 — es lo que hace seguro
   > bajar DLLs que el juego EJECUTA, sobre todo por P2P (peers no confiables). Sacarla a secas
   > REMUEVE esa garantia. Solo tiene sentido si se reemplaza el ancla de confianza, p.ej. con el
   > feature 3 (login GitHub): el manifest viene del repo AUTENTICADO del publicador via HTTPS, y el
   > content-addressing por BLAKE3 garantiza integridad. Hay que decidir el modelo nuevo antes de
   > implementar (que pasa con P2P, donde el peer no es GitHub). A discutir al llegar aca.

3. **Login de GitHub en la app + crear repo publico de mods automatico.** ✅ **HECHO (1.2.0).**
   Modulo `github`: login con PAT pegado o OAuth device-flow (con `OAUTH_CLIENT_ID` configurable),
   token guardado SEGURO en el llavero del SO (`keyring`). `publish` sube por la API REST (crea el
   repo publico si falta, crea/usa el release, sube con clobber el manifest+firma+torrent+assets) sin
   el `gh` CLI (fallback a `gh` si no hay login). GUI (pestaña Publicar) + CLI (`github-login/-status/
   -logout`). Es el nuevo ancla de confianza para el feature 2 (proximo).
