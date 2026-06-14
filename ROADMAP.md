# Roadmap a 1.0.0 — sts2-modsync

> Plan derivado de una auditoria del codigo en 6 dimensiones (features/UX, robustez, testing,
> seguridad, distribucion, performance). Convencion del repo: espanol ASCII sin tildes.

## Orden de ejecucion (acordado)

1. **Quick wins / Fase 0.3** (red de seguridad) — se empieza por aca.
2. **Todo el ROADMAP** (fases 0.4 -> 1.0).
3. **Features post-1.0** (ver seccion final): single `.exe`, sacar dependencia del `.minisig`,
   login de GitHub en la app + crear repo publico de mods automatico.

## 1. Donde estamos

0.2.3 **funcionalmente completo pero NO en grado 1.0**: el flujo central (detectar, mod manager,
sync transaccional firmado, publish, auto-update, P2P) funciona. Lo que falta NO son features, es
**confianza/estabilidad/UX para no-tecnicos**. Las dimensiones duras (robustez, testing, seguridad,
distribucion) tienen bloqueantes; "Features y UX" no tiene ninguno.

Agujeros sistemicos: **(a)** no hay CI de test/lint (solo release por tag) -> un tag puede
auto-distribuir una regresion via auto-update; **(b)** los modulos peligrosos (`manager.rs`,
`transport.rs`, el `apply` del auto-update que EJECUTA un binario) tienen **cero tests**; **(c)**
falta LICENSE. Mas dos fallas de integridad: rename no atomico ante fallo parcial en `sync::apply`
y `is_game_running` fragil.

## 2. Criterios de 1.0.0 (Definition of Done)

- [x] CI en push/PR: `fmt --check` + `clippy -D warnings` + `cargo test` + `build --features gui`,
      y el mismo gate ANTES de `gh release create`. **(0.3)**
- [x] `manager.rs` con tests (enable/disable, uninstall, `install_from_zip`, `safe_id`, zip-slip). **(0.3)**
- [x] Auto-update con tests (`extract_named`, `release_from_json`, filtro de tags `v*`). **(0.3)**
- [ ] `transport.rs` con tests (mock loopback: Range 206/200, tamano final, `join_url`).
      (Parcial: `join_url`/`require_https` testeados; falta el mock loopback de Range — ver fase 0.4.)
- [x] `sync::apply` realmente transaccional (rename con backup+rollback). **(0.2.4)**
- [x] `is_game_running` robusto (nunca mutar `mods/` con el juego abierto). **(0.2.4)**
- [x] Errores nunca tragados (huerfanos no borrados se reportan; hash-mismatch reintenta). **(0.2.4)**
- [x] Seguridad enforced en codigo (`http://` rechazado; zip-slip del install local cerrado). **(0.2.4)**
- [x] LICENSE + `license=` en Cargo.toml + README de usuario final (aviso SmartScreen). **(0.2.6)**
- [x] Auto-update recuperable (`.bak` del exe viejo + verificar arranque). **(0.2.6)**
- [x] Logging a archivo en %APPDATA% + panic-hook (el GUI no tiene consola). **(0.2.6)**
- [x] Config versionada (no perder `install_root`/`subscribed_sets` en silencio). **(0.2.6)**
- [ ] Cancelacion + progreso detallado en sync/publish/install.
- [ ] Feedback de UI honesto (`install_note` se renderiza; firma visible/afirmativa).

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

### 0.7 -> 1.0 — Pulido de producto (UX) · effort medio-alto
- Cancelacion de sync/publish/install + limpieza al cancelar.
- Progreso detallado (archivo actual + velocidad/ETA) + throttle del repaint.
- Arreglar `install_note` + onboarding (explicar BaseLib/ModListSorter/orden de carga).
- Toasts por-pestana con auto-dismiss + errores accionables.
- Lista de Mods con ordenamiento/filtros + boton "habilitar dependencias faltantes".
- Sets suscritos con nombre legible + indicador "version nueva disponible".
- Cache de hashes (path -> size+mtime+blake3) para no re-hashear GB.

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

1. **Un solo `.exe` para ejecutar.** Unificar en un unico binario portable (hoy se compilan dos:
   `sts2-modsync-gui.exe` + `sts2-modsync.exe`). Objetivo: el usuario baja un solo archivo y lo
   corre, sin CLI aparte ni dependencias externas (hoy `publish` depende del `gh` CLI — ver feature 3).

2. **Sacar la dependencia del `.minisig`.**
   > **OJO (decision de seguridad):** la firma minisign es el invariante P0 — es lo que hace seguro
   > bajar DLLs que el juego EJECUTA, sobre todo por P2P (peers no confiables). Sacarla a secas
   > REMUEVE esa garantia. Solo tiene sentido si se reemplaza el ancla de confianza, p.ej. con el
   > feature 3 (login GitHub): el manifest viene del repo AUTENTICADO del publicador via HTTPS, y el
   > content-addressing por BLAKE3 garantiza integridad. Hay que decidir el modelo nuevo antes de
   > implementar (que pasa con P2P, donde el peer no es GitHub). A discutir al llegar aca.

3. **Login de GitHub en la app + crear repo publico de mods automatico.** OAuth device-flow desde
   la app (es desktop), guardar el token de forma segura, y via la API de GitHub crear el repo
   publico de sets, crear releases y subir assets DIRECTAMENTE (sin `gh` CLI, sin que el usuario
   toque nada). Esto reemplaza el `gh` de `publish::upload` y la creacion manual del repo, y puede
   ser el nuevo ancla de confianza del feature 2.
