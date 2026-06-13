# sts2-modsync

**Mod manager de *Slay the Spire 2*** (Windows / Rust): detecta el install (Steam o copias
**fuera de Steam**), **lista / habilita / deshabilita / instala / desinstala** mods, gestiona
**perfiles** y el **orden de carga**, y **lanza** el juego. La **sincronizacion de sets** entre
un modder y sus amigos (gratis y rapida, por hash) es **un modulo mas**. GUI + CLI.

> Estado: mod manager funcional (GUI con pestañas Mods/Sync/Perfiles + CLI). La sync ya **baja e
> instala de verdad** (`apply` transaccional + descarga de GitHub Releases, verificada por hash);
> el delta intra-`.pck` es FASE 3 — ver [HANDOFF.md](HANDOFF.md).

## Como funciona (resumen)

- **Detecta** StS2 via Steam (AppID 2868840) o, si no, por rutas comunes; si nada, abre
  un dialogo para que elijas la carpeta (copias pirata). Valida que sea StS2 de verdad
  (`SlayTheSpire2.exe` + `data_sts2_windows_x86_64/`), no por el nombre.
- Un **set** de mods se publica como un *GitHub Release* (gratis, sin login para bajar,
  hasta 2 GiB por archivo) con un **manifiesto** que lista cada mod, sus archivos, tamano
  y hash **BLAKE3**.
- El cliente baja **solo lo que cambio** (compara hashes) — no rebaja los `.pck` de 100+ MB
  que no se tocaron. Respeta los mods que el amigo tenga por su cuenta (solo gestiona las
  carpetas listadas en el set).
- Seguridad: el manifiesto se **firma** (minisign) y cada archivo se **verifica por hash**;
  la app exige el juego cerrado y aplica los cambios de forma transaccional.

## Uso

```sh
# GUI (mod manager con pestañas Mods / Sync / Perfiles):
cargo run --features gui --bin sts2-modsync-gui

# CLI:
cargo run -- list                 # lista los mods instalados (default)
cargo run -- enable  <id>         # habilita un mod (mueve la carpeta a mods/)
cargo run -- disable <id>         # deshabilita un mod (a mods_disabled/)
cargo run -- launch               # lanza el juego
cargo run -- sync set-manifest.example.json   # dry-run del plan de sincronizacion
cargo run -- publish --name "Mi Set" --version 0.0.1 \
  --base-url https://github.com/USER/REPO/releases/download/0.0.1/ --out ./pub   # (modder)
```

## Build

Requiere Rust (toolchain MSVC) + Visual Studio Build Tools.

```sh
cargo build            # debug
cargo build --release  # binario optimizado (~8-15 MB, un solo .exe)
cargo test
```

## Estructura

| archivo | que hace |
|---|---|
| `src/detect.rs` | encontrar/validar el install (Steam + pirata + dialogo) |
| `src/modlist.rs` | escanear/parsear los mods instalados (`<id>.json`), deps, orden de carga |
| `src/manager.rs` | enable/disable/instalar/desinstalar (mover carpetas; juego cerrado) |
| `src/profile.rs` | perfiles (conjuntos de mods habilitados) + puente con la sync |
| `src/launch.rs` | lanzar el juego |
| `src/manifest.rs` | modelo del **set-manifest** (sync) + validacion + orden de dependencias |
| `src/hashing.rs` | BLAKE3 por archivo (delta gruesa) |
| `src/sync.rs` | plan (bajar / al dia / huerfanos) + `apply` transaccional (baja/verifica/instala) |
| `src/publish.rs` | generar set-manifest + assets desde tus mods (modo modder) |
| `src/signing.rs` | verificar la firma minisign del manifiesto |
| `src/transport.rs` | descarga de GitHub Releases (reqwest blocking, content-addressed por blake3) |
| `src/config.rs` | config local (`%APPDATA%/sts2-modsync/config.toml`) |
| `src/main.rs` | CLI con subcomandos (`list/enable/disable/launch/sync`) |
| `src/gui.rs` | GUI mod manager: pestañas Mods/Sync/Perfiles (feature `gui`, bin `sts2-modsync-gui`) |

Detalles de arquitectura, decisiones y proximos pasos: **[HANDOFF.md](HANDOFF.md)**.
