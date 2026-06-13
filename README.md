# sts2-modsync

Herramienta de Windows (Rust) para **detectar** el install de *Slay the Spire 2* —
incluyendo copias **fuera de Steam** (te deja elegir la carpeta) — y **sincronizar sets
de mods** entre un modder y sus amigos, de forma **gratis y rapida**.

> Estado: **FASE 1** (core + MVP de linea de comandos). La descarga real, la GUI y el
> delta del `.pck` son FASE 2 — ver [HANDOFF.md](HANDOFF.md).

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

## Uso (MVP actual)

```sh
cargo run                          # detecta el install y lo reporta
cargo run -- set-manifest.example.json   # ademas muestra el PLAN de sync (dry-run)
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
| `src/manifest.rs` | modelo del set-manifest + validacion + orden de dependencias |
| `src/hashing.rs` | BLAKE3 por archivo (delta gruesa) |
| `src/sync.rs` | calcular el plan (bajar / al dia / huerfanos); `apply` = FASE 2 |
| `src/signing.rs` | verificar la firma minisign del manifiesto |
| `src/transport.rs` | trait `ModSource`; impl GitHub Releases = FASE 2 |
| `src/config.rs` | config local (`%APPDATA%/sts2-modsync/config.toml`) |
| `src/main.rs` | MVP de linea de comandos |

Detalles de arquitectura, decisiones y proximos pasos: **[HANDOFF.md](HANDOFF.md)**.
