# Changelog

Formato basado en [Keep a Changelog](https://keepachangelog.com/). Mientras estemos en 0.x, los
cambios incompatibles pueden ocurrir en cualquier release.

## [0.2.7] - 2026-06-14 — Pulido de producto / UX (fase 0.7)

- **Cache de hashes** (`%APPDATA%\sts2-modsync\hashcache.json`): no re-hashea los `.pck` de 100+ MB
  en cada `plan()` si no cambiaron (compara size+mtime). Mucho mas rapido abrir la pestaña Sync.
- **Cancelacion** de la sincronizacion (boton Cancelar), incluso a mitad de una descarga grande;
  no instala nada y deja los `.part` para reanudar.
- **Progreso detallado:** archivo actual, MB bajados/total, velocidad y ETA; repaint throttled.
- **Onboarding:** explicacion colapsable de BaseLib / ModListSorter / orden de carga (multiplayer).
- **Lista de Mods:** toggle "habilitados primero" + boton "habilitar dependencias ya instaladas".
- **Sets guardados:** nombre legible (en vez de la URL cruda) + "Buscar actualizaciones" que marca
  los que tienen una version mas nueva publicada.
- **Toasts:** los avisos de exito se descartan solos; los errores quedan con un hint accionable.

## [0.2.6] - 2026-06-14 — Distribuible y diagnosticable (fase 0.6)

- **Auto-update RECUPERABLE:** respalda el exe viejo (`.bak`), verifica que el nuevo arranca
  (`--health-check`) y, si no arranca, vuelve a la version anterior automaticamente.
- **Logging + panic-hook:** se escribe a `%APPDATA%\sts2-modsync\sts2-modsync.log` (un crash del
  GUI, que puede no tener consola, deja rastro con backtrace). Rota al pasar 1 MiB.
- **Config versionada y a prueba de corrupcion:** campo `schema`; una config invalida se respalda
  en `.toml.bad` en vez de resetearse en silencio (no se pierde `install_root`/`subscribed_sets`).
- Las **notas del release** se muestran antes de actualizar (GUI y CLI).
- README con seccion para usuarios finales (link al release, single-exe, aviso SmartScreen).

## [0.2.5] - 2026-06-14 — Seguridad de la cadena (fase 0.5)

- HTTPS obligatorio en CADA descarga (manifest, firma, assets, zip+`.minisig` del auto-update).
- Verificacion de firma VISIBLE y afirmativa (verde "verificada" / naranja "modo dev").
- `cargo-audit` en CI; `SECURITY.md`; tests negativos de seguridad.

## [0.2.4] - 2026-06-14 — Integridad transaccional (fase 0.4)

- `apply` transaccional con **backup + rollback**: el set nunca queda a medio aplicar.
- Errores que no se tragan (huerfanos no borrados se reportan; reintento de descarga de cero).
- `is_game_running` endurecido; validacion del `id` del manifest; pre-check de disco; resume que
  re-baja de cero si el `.part` quedo corrupto; soporte de long-paths en Windows.

## [0.2.3] - 2026-06-14

- Sync P2P estilo torrent (librqbit) + fallback HTTP.

## Anteriores (0.1.0 – 0.2.2)

Ver el historial de git y los [GitHub Releases](https://github.com/YX14ng/sts2-modsync/releases).
