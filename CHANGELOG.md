# Changelog

Formato basado en [Keep a Changelog](https://keepachangelog.com/). Mientras estemos en 0.x, los
cambios incompatibles pueden ocurrir en cualquier release.

## [1.11.0] - 2026-06-15 — auto-update sin minisign · sync P2P opt-in (no mas 0%) · modo claro

- **El auto-update ya NO exige firma minisign.** El ancla de confianza pasa a ser HTTPS + que el
  release viene del repo del dueño (estandar para auto-update) + el `--health-check` con rollback al
  `.bak` antes de relanzar. `update::apply` ya no baja ni verifica el `.minisig`; el CI ya no firma el
  binario. Nadie necesita una clave minisign ni para publicar ni para actualizar. (La firma OPCIONAL
  de los set-manifests de sync sigue igual: si esta se valida, si no, se acepta por HTTPS + BLAKE3.)
- **Sync: P2P (torrent) ahora es OPT-IN — se arregla el "se queda en 0%".** Un set publicado trae un
  `magnet`, y el cliente intentaba P2P primero: si el publicador no estaba seedeando, `add_torrent`
  colgaba resolviendo la metadata del swarm y la barra quedaba en 0% PARA SIEMPRE (el timeout solo
  cubria el poll posterior). Ahora la sync baja por **HTTP por default** (GitHub Releases, siempre
  disponible) y solo intenta P2P si se opta con `STS2_P2P=1` (o peers manuales). Ademas se le puso un
  timeout a la resolucion de metadata, asi aun optando por P2P cae a HTTP si no hay seeder.
- **Modo claro rediseñado.** El tema claro tenia el gris plano de egui y un acento lavado. Ahora tiene
  paleta propia: superficies cohesivas (central gris, cards blancas que resaltan, inputs apenas
  grises), texto slate oscuro legible y seleccion con contraste. El acento es un azul royal que se lee
  bien en ambos temas.

## [1.10.0] - 2026-06-15 — GitHub: elegir/crear repo con un clic · Nexus Premium: actualizar directo

- **GitHub — elegir o crear el repo de publicacion sin tipearlo** (pestaña Publicar, con sesion de
  GitHub iniciada): un combo lista tus repos (los que podes pushear) para elegir uno, y un campo
  "crear repo" arma uno PUBLICO nuevo bajo tu cuenta. Lo elegido se recuerda (`config.publish_repo`)
  al instante. Nuevo en la API: `github::Api::list_repos` (pagina `/user/repos`, filtra por push) y
  `create_repo` (POST `/user/repos`, devuelve `owner/repo`; 422 = ya existia).
- **Nexus Premium — actualizar mods DIRECTO** (sin el handler `nxm://`): si tu cuenta es Premium, al
  buscar actualizacion de un mod de Nexus aparece "Actualizar (Premium)" que resuelve el archivo MAIN
  (`nexus::latest_main_file`), baja el `.zip` por la API (`download_link` directo, sin `key/expires`)
  e instala reemplazando (solo si el zip declara ese mismo id). Las cuentas gratis siguen con "Mod
  Manager Download" (`nxm://`). La app valida la API key guardada al abrir para saber si sos Premium.
- CLI: `mod-update <id>` ahora tambien actualiza mods de Nexus si la cuenta es Premium.
- `.7z`/`.rar` de Nexus no se auto-instalan (se avisa para bajarlos a mano), igual que el flujo `nxm`.

## [1.9.0] - 2026-06-15 — Nexus: descarga automatica via handler nxm:// (auto-update fase 2b)

- **Descarga automatica de mods de Nexus** (modulo `nxm`): se registra la app como handler del
  protocolo `nxm://` (boton "Mod Manager Download" de la web de Nexus). Cuando lo tocas en la pagina
  de un mod, el navegador le pasa el link a la app, que resuelve el download-link (`nexus::download_link`,
  con el `key`/`expires` de un solo uso para usuarios gratis, o directo si sos Premium), baja e instala.
- **GUI:** boton "Registrar Mod Manager Download (nxm://)" en el detalle de un mod de Nexus (+ quitar).
  **CLI:** `nxm-register` / `nxm-unregister` (alta/baja del handler), `nxm <link>` (lo invoca Windows).
- Como `nxm <link>` lo lanza el protocolo (sin consola), el resultado se muestra en un **dialogo** del SO.
- Solo se instalan **`.zip`** automaticamente; si Nexus sirve `.7z`/`.rar`, se guarda en Descargas con un
  aviso para instalarlo a mano (extraer + "Instalar carpeta"/".zip"). El install reusa la defensa
  anti zip-slip y exige el juego cerrado.
- Registrar `nxm://` TOMA el protocolo de Vortex/Mod Organizer si los tenes (es opt-in y reversible).
- El handler escribe en `HKCU` (per-user, sin admin). Descarga con tope de tamaño y HTTPS en cada hop.

> El flujo end-to-end (web -> app) necesita una cuenta de Nexus real para probarse; los componentes
> (parseo del link, registro del protocolo, descarga, install) tienen tests. Cierra la fase 2 del
> auto-update de mods: GitHub (1.7) + Nexus chequeo (1.8) + Nexus descarga (1.9).

## [1.8.0] - 2026-06-15 — Nexus: API key + chequeo de version (auto-update fase 2a)

- **Conexion con Nexus Mods** (modulo `nexus`): pegas tu **API Key personal** (de tu cuenta, en
  Preferences -> API) y se guarda SEGURO en el llavero del SO (como el token de GitHub). CLI:
  `nexus-login` / `nexus-status` / `nexus-logout`. GUI: campo "API Key de Nexus" en el detalle de un
  mod de Nexus.
- **Chequeo de version de mods de Nexus:** "Buscar actualizacion" / `mod-check` ahora consultan la API
  de Nexus (`/v1/games/{game}/mods/{id}.json`) y muestran la version disponible, no solo "abrir".
- **La DESCARGA automatica de Nexus sigue siendo fase 2b** (handler `nxm://`): por ahora, cuando hay
  version nueva, el boton es "Abrir en Nexus para bajar" (Nexus exige el flujo nxm para usuarios gratis,
  o Premium para el link directo). El chequeo de version SI funciona para todos con la API key.
- Nota: Nexus no tiene un canal "beta" formal, asi que el toggle estable/beta solo aplica a GitHub;
  para Nexus se usa la version headline del mod.

## [1.7.0] - 2026-06-15 — Auto-update de mods desde su upstream (GitHub) · fase 1

- **Cada mod puede tener un ORIGEN** (su repo de GitHub o su pagina de Nexus) y el programa
  **chequea/baja la version nueva** y la instala (reemplaza preservando si estaba habilitado o no).
- **Canal BETA vs MAIN global:** un switch ("Canal beta") elige seguir pre-releases (BETA) o solo
  releases estables (MAIN). En GitHub el mapeo es limpio: BETA = `prerelease`, MAIN = release estable.
- **De donde sale el origen:** del `<id>.json` del mod si trae `repository`/`url`/`homepage`, o lo
  **pegas vos** en el detalle del mod (usuario/repo o URL de Nexus) — se recuerda en `config.mod_sources`.
- **GitHub: auto-update completo** (gratis, sin login). **Nexus: fase 1 solo chequeo/abrir la pagina**
  — la descarga automatica de Nexus necesita Premium o el handler `nxm://`, que llega en la **fase 2**.
- GUI (pestaña Mods): en el detalle del mod, el origen + "Buscar actualizacion" + "Actualizar".
  CLI: `mod-source <id> <usuario/repo|URL>`, `mod-check [<id>]`, `mod-update <id>`.

> Seguridad: la actualizacion baja un `.zip` por HTTPS y lo extrae con la misma defensa anti
> zip-slip que el install manual; no hay firma por-mod (el ancla de confianza es el repo upstream que
> VOS elegiste como origen, igual que bajar el mod a mano de ahi).

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
  un binario — ese vector NO se relajo. _(Nota: en v1.11.0 esto se removio — el auto-update dejo de
  exigir firma; ancla HTTPS + repo del dueño + `--health-check` con rollback. Ver el entry de 1.11.0.)_

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
