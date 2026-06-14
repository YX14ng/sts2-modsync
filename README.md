# sts2-modsync

**Mod manager de *Slay the Spire 2*** (Windows / Rust): detecta el install (Steam o copias
**fuera de Steam**), **lista / habilita / deshabilita / instala / desinstala** mods, gestiona
**perfiles** y el **orden de carga**, y **lanza** el juego. La **sincronizacion de sets** entre
un modder y sus amigos (gratis y rapida, por hash) es **un modulo mas**. GUI + CLI.

> Estado: **1.0.0** — estable. Mod manager + sync transaccional firmada + auto-update recuperable +
> P2P, con CI, logging, y UX pulida para no-tecnicos (ver [CHANGELOG.md](CHANGELOG.md) y
> [ROADMAP.md](ROADMAP.md)). El delta intra-`.pck` (re-bajar solo el trozo cambiado de un `.pck`)
> queda fuera de 1.0 — ver [HANDOFF.md](HANDOFF.md).

## Instalar (usuarios finales)

1. Baja el ultimo **[Release](https://github.com/YX14ng/sts2-modsync/releases/latest)**:
   `sts2-modsync-windows-x86_64.zip`.
2. Descomprimi y corre **`sts2-modsync.exe`** — es **un solo ejecutable** portable, sin instalador.
   Doble-clic lo abre como app (GUI); el MISMO `.exe` con subcomandos es la CLI
   (`sts2-modsync.exe list`, `... publish ...`, etc.).
3. La primera vez detecta Slay the Spire 2 solo (Steam o por rutas comunes); si no lo halla, te
   abre un dialogo para elegir la carpeta del juego.
4. La app se **auto-actualiza** sola desde GitHub Releases (verifica firma y que el binario nuevo
   arranque antes de reemplazarse; si falla, vuelve a la version anterior).

> **Aviso de SmartScreen:** el binario no esta firmado con Authenticode (un certificado pago), asi
> que Windows SmartScreen puede mostrar *"Windows protegio tu PC / editor desconocido"*. Es
> esperable. Para correrlo: **Mas informacion -> Ejecutar de todas formas**. El `.zip` del release
> incluye su firma **minisign** (`.zip.minisig`) por si queres verificarlo. Si algo falla, hay un
> log en `%APPDATA%\sts2-modsync\sts2-modsync.log`.

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
cargo run --features gui          # sin argumentos -> abre la GUI (single-exe)

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
| `src/gui.rs` | GUI mod manager: pestañas Mods/Sync/Perfiles (feature `gui`; la abre `main` sin args) |

Detalles de arquitectura, decisiones y proximos pasos: **[HANDOFF.md](HANDOFF.md)**.
