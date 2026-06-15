# Changelog

Formato basado en [Keep a Changelog](https://keepachangelog.com/). Mientras estemos en 0.x, los
cambios incompatibles pueden ocurrir en cualquier release.

## [1.6.0] - 2026-06-15 — Delta intra-`.pck` (al actualizar 1 mod, solo baja el diff)

- **Update incremental DENTRO de un archivo:** si cambiás una carta de un mod, tus amigos que ya
  tienen la version vieja del `.pck` **bajan solo el diff** (un patch bsdiff), no el `.pck` de 100 MB
  entero. Es el ultimo pedazo que faltaba para que "actualizar un mod" sea verdaderamente minimo.
- **publish** genera los patches contra la **publicacion anterior** que tengas en la carpeta de
  salida (`set-manifest.json` viejo + `assets/`), y los sube como assets content-addressed. Cero
  friccion si reusas la misma carpeta `--out`. `--no-delta` lo desactiva. Un patch se descarta si no
  resulta mas chico que el full.
- **sync** elige el patch cuando el archivo local viejo matchea (por BLAKE3) un `delta.from_blake3`
  del manifest y el patch es mas chico; lo baja, lo aplica, y **verifica el BLAKE3 del resultado**.
- **Seguro por construccion:** el patch es un asset content-addressed (su hash se verifica al bajarlo)
  y el resultado de aplicarlo se re-verifica contra el `blake3` del manifest. Si algo falla (patch
  corrupto, el archivo viejo cambio, etc.) la sync **cae a bajar el asset completo** — un delta nunca
  puede instalar bytes equivocados ni romper la transaccion (sigue siendo `.part` + rename atomico).
- Implementado con `qbsdiff` (bsdiff, pure-Rust salvo una dep C que compila en MSVC). Tope de tamaño
  por las dudas (genera deltas hasta 600 MB, los aplica hasta 512 MB; arriba de eso, full en streaming).

## [1.5.0] - 2026-06-15 — Suscribirse a un REPO (sigue el ultimo release)

- Ahora podes **suscribirte a un repo** (`usuario/repo`) en vez de a la URL de un release fijo. El
  programa **resuelve el ULTIMO release** (`GET /releases/latest`, sin login) en cada "Buscar
  actualizaciones" / re-sync, asi cuando el modder publica un release nuevo (con `publish`, que sube
  otro release al mismo repo desde 1.4.0) tus amigos lo ven **sin tener que re-pegar la URL**.
- Combinado con el delta por BLAKE3 que ya existia: al actualizar, **solo se baja lo que cambio**
  (los `.pck` que no cambiaron no se vuelven a bajar; el delta DENTRO de un `.pck` sigue siendo fase 3).
- GUI (pestaña Sync): campo **"o Repositorio: usuario/repo"** + boton "Suscribirse". Los sets guardados
  muestran "owner/repo (ultimo release)" para las suscripciones por repo. Las suscripciones por URL
  fija de antes **siguen funcionando igual** (no hay migracion forzada).
- CLI: `sync owner/repo` (o `sync repo:owner/repo`) hace el dry-run del plan resolviendo el ultimo release.

> Nota: la resolucion usa la API anonima de GitHub (60 req/hora) — alcanza de sobra para chequeos
> manuales. La descarga de assets sigue siendo por el CDN directo (sin tocar la API), via el
> `base_url` que el manifest firmado trae para ese release.

## [1.4.0] - 2026-06-14 — Recordar el repo de publicacion (no recrear repos)

- La app **RECUERDA el repositorio** donde publicaste tus mods (`config.publish_repo`): "actualizar
  la lista de mods" ahora es **subir OTRO release al MISMO repo**, no crear un repo nuevo cada vez.
- GUI (pestaña Publicar): el campo crudo `base_url` se reemplazo por **"Repositorio:" (usuario/repo)**,
  pre-cargado con lo ultimo que publicaste; un hint dinamico muestra exactamente a donde va
  (`→ release '<tag>' en github.com/<owner>/<repo>`) y avisa que actualizar = otro release, no otro repo.
- CLI: `publish` acepta **`--repo <owner/repo>`** (ademas del `--base-url` legacy) y, si lo omitis,
  reusa el repo recordado. El `base_url` de descarga se deriva siempre como `https://...` (no hay
  forma de degradar a `http://`). El nombre del set tambien se recuerda para pre-cargar el form.
- **Saneo del input** (`github::normalize_repo` / `github::valid_tag`): el repo se normaliza
  (saca `?query`/`#fragment` de una URL pegada, trimea, valida el charset real de GitHub) y la
  version/tag se valida (sin espacios, sin `/`, sin `..`, charset seguro) ANTES de armar el
  `base_url`. Esto evita que basura termine en el `base_url` que queda firmado dentro del
  set-manifest que bajan los amigos, o que un tag con `/` rompa el round-trip y de 404.

## [1.3.0] - 2026-06-14 — Firma minisign opcional para sets (post-1.0 #3)

- La **firma minisign de un set-manifest ya NO es obligatoria** (`signing::verify_optional`): el
  ancla de confianza es que bajaste el manifest por **HTTPS desde el repo del publicador** que un
  amigo te paso (autenticado por GitHub) y que cada asset se verifica por **BLAKE3**. Un publicador
  ya no NECESITA manejar una clave minisign para compartir sets.
- Si un set viene firmado, se verifica (capa extra) y una firma **invalida se rechaza** (tampering);
  si no, se acepta como "sin firma". La UI lo muestra claro: verde "✓ Firma verificada" / naranja
  "● Sin firma: confias en la URL/HTTPS"; la CLI (`sync`) imprime el estado.
- El **auto-update sigue exigiendo firma** (`verify_with_embedded`, estricto) porque baja y EJECUTA
  un binario — ese vector NO se relajo.

> Trade-off: un set sin firma confia en que la cuenta de GitHub del publicador no este comprometida
> (la firma protegia contra eso). Firmar sigue siendo recomendado; ahora es opcional.

## [1.2.0] - 2026-06-14 — Publicar sin el `gh` CLI (post-1.0 #2)

- **Login de GitHub en la app** (modulo `github`): se puede conectar con un **Personal Access
  Token** (pegado) o por **OAuth device-flow** (si se configura `github::OAUTH_CLIENT_ID`). El
  token se guarda SEGURO en el llavero del SO (Credential Manager en Windows) via `keyring`,
  nunca en texto plano.
- **`publish` sube por la API REST de GitHub** cuando hay login: crea el repo publico si falta,
  crea/usa el release del tag, y sube (con clobber) el manifest + firma + torrent + assets — sin
  depender del `gh` CLI. Sin login, sigue cayendo al `gh` CLI como fallback.
- GUI: seccion "Conectar con GitHub" en la pestaña Publicar (PAT o device-flow, con estado).
- CLI: `github-login <token>` / `github-status` / `github-logout`.

## [1.1.0] - 2026-06-14 — Un solo ejecutable (post-1.0 #1)

- **Single-exe:** ahora hay UN solo binario `sts2-modsync.exe` (antes eran dos:
  `sts2-modsync-gui.exe` + `sts2-modsync.exe`). Doble-clic (sin argumentos) lo abre como app
  (GUI); el mismo `.exe` con subcomandos es la CLI (`list`, `publish`, `sign`, etc.).
- En Windows usa el subsistema `windows` (no abre una consola negra al lanzar el GUI) y, en modo
  CLI, se engancha a la consola del padre (`AttachConsole`) para que la salida sea visible desde
  una terminal. El build liviano sin la feature `gui` sigue siendo una CLI de consola normal.
- El auto-update y los workflows de CI/release ahora producen/consumen ese unico `.exe`.

> **Migracion desde 1.0.0:** el auto-update de 1.0.0 buscaba `sts2-modsync-gui.exe` en el zip del
> release; el zip de 1.1.0 ya no lo trae, asi que **quien este en el .exe de 1.0.0 tiene que bajar
> 1.1.0 a mano una vez**. Desde 1.1.0 el auto-update vuelve a funcionar solo.

## [1.0.0] - 2026-06-14 — Primera version estable

Cierre del roadmap a 1.0: el flujo central (detectar, mod manager, sync transaccional firmado,
publish, auto-update recuperable, P2P) es robusto, seguro, diagnosticable y comodo para
no-tecnicos. Las fases 0.4 → 0.7 (integridad transaccional, seguridad de la cadena,
distribuible/diagnosticable, pulido UX) estan completas y revisadas adversarialmente.

- Ultimo item del Definition of Done cerrado: tests de `transport.rs` con un mock loopback que
  ejercita la descarga full (200) y el resume con HTTP Range (206) + chequeo de tamano final.
- `require_https` ahora permite `http://` SOLO a loopback (127.0.0.1 / localhost / [::1]): ese
  trafico no sale de la maquina (no hay MITM) y habilita mirrors/tests locales.

Ver las entradas 0.2.4–0.2.7 para el detalle de cada fase.

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
