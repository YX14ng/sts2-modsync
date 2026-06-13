# CLAUDE.md

Guia para Claude Code en este repo. **Antes de tocar codigo, lee [HANDOFF.md](HANDOFF.md)** —
tiene el research (transporte/costo, stack, deteccion, seguridad) ya hecho y el plan de fases.

## Que es

`sts2-modsync` — app **Rust para Windows** que detecta el install de *Slay the Spire 2*
(Steam o copias pirata via dialogo de carpeta) y **sincroniza sets de mods** entre un
modder y sus amigos, gratis (GitHub Releases) y rapido (solo baja lo que cambio por hash).

## Estado

- **FASE 1 (hecha, compila):** core agnostico de GUI/red — deteccion, modelo de manifiesto,
  hashing BLAKE3, planificador de sync, verificacion de firma, trait de transporte + MVP CLI.
- **FASE 2 (siguiente):** transporte real (reqwest+GitHub Releases), `apply()` transaccional,
  GUI (eframe). **FASE 3:** delta intra-`.pck` (bita), auto-update. Detalle en HANDOFF.md.

Hoy `sync::apply` y `GitHubReleases::fetch` son **stubs que hacen `bail!`** a proposito (no
son bugs) — su doc-comment es el contrato de FASE 2. `signing::PUBLISHER_PUBKEY` vacia =
**modo dev** (firma NO verificada). Las deps de FASE 2 estan **comentadas** en `Cargo.toml`.

## Arquitectura (modulos en `src/`)

`detect` (Steam/pirata + validacion + juego-abierto) · `manifest` (set-manifest + validacion
de paths + toposort de deps) · `hashing` (blake3 mmap+rayon) · `sync` (`plan()` listo;
`apply()` = FASE 2) · `signing` (minisign) · `transport` (trait `ModSource`; GitHub = FASE 2)
· `config` (%APPDATA%) · `main` (CLI). El binario es una cascara sobre la lib (`lib.rs`).

El **set-manifest** (`manifest.rs` / `set-manifest.example.json`) es un artefacto NUEVO que
describe un *set* entero para sincronizar — **NO** es el `<Id>.json` que cada mod trae para
el juego. No los confundas.

## Comandos

- `cargo run -- set-manifest.example.json` — detecta StS2 + dry-run del plan.
- `cargo test` · `cargo clippy` · `cargo fmt` · `cargo build --release`.
- Agregar deps: `cargo add <crate> --features ...` (NO hardcodear versiones a ojo — el
  research alucino algunos patch; deja que cargo resuelva).

## Invariantes que NO romper

- **Seguridad (baja DLLs que el juego ejecuta):** firma del manifiesto (P0) + hash por
  archivo + HTTPS. Ver §seguridad de HANDOFF.
- **Nunca tocar carpetas fuera de `manifest.managed_ids()`** (no pisar mods ajenos del amigo).
- **Path-traversal:** `manifest::validate_paths` rechaza `..`/absolutas — mantenerlo.
- **Apply transaccional:** todo a `.part` + verificado, luego renames atomicos juntos; abortar
  si el juego corre (lock de `.dll/.pck` en Windows).

## Convenciones

- Config local: `config.local.toml` / `%APPDATA%/sts2-modsync/config.toml` (gitignorado).
  Plantilla no-secreta: `config.example.toml`.
- No versionar blobs (`.pck`, `.dll`, `*.pdb`) ni `/test-mods` (gitignorado).
- El autor escribe en **espanol, sin tildes ni diacriticos** (ASCII: `deteccion`, no
  `detección`); igualar el idioma y ese estilo al editar comentarios/docs.
