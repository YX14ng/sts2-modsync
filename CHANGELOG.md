# Changelog

Formato basado en [Keep a Changelog](https://keepachangelog.com/). Mientras estemos en 0.x, los
cambios incompatibles pueden ocurrir en cualquier release.

## [1.23.0] - 2026-06-16 — pulido de UX (el amigo que recibe, el modder que publica)

- **Pestaña Sync: UN solo campo que detecta solo.** Pegas un LINK (`https://...`) o un `usuario/repo`
  y "Cargar" hace lo correcto (URL directa, o suscribirse al ultimo release del repo) — antes habia
  que saber en cual de dos campos iba cada cosa. Ademas una linea que EXPLICA que pegar (el tab que
  menos guiaba al amigo no-tecnico que recibe un set).
- **Publicar: link directo para crear el token de GitHub** con el scope `public_repo` ya marcado
  (crear un PAT a mano era la parte confusa).
- **Pestaña Mods: botones "Abrir carpeta de mods" y "Abrir datos/log"** (antes los errores te pedian
  navegar a `%APPDATA%` a mano), y **"Habilitar todos" / "Deshabilitar todos"** para cuando hay muchos
  mods (reversible, tolera fallos por mod y reporta cuantos).
- **Perfiles: guardar sobre un perfil que ya existe ahora PIDE confirmacion** (sobrescribir reescribe
  el archivo, no va a la papelera).
- **Jugar por Steam:** si tocas Jugar y no abre, el aviso te dice como pasar a modo directo (antes
  quedaba un "lanzando..." y nada).

## [1.22.0] - 2026-06-16 — limpieza interna (sin cambios de comportamiento)

Pasada de salud de codigo salida de la auditoria: consolida logica duplicada para que no diverja.
Sin cambios visibles para el usuario; cubierto por tests + review de equivalencia.

- **Un solo dispatch de chequeo de updates** (`modupdate::check` + `CheckCtx`): antes los dos workers
  del GUI (un mod / todos) repetian el `match GitHub/Nexus` y diferian en el guard de Nexus. Ahora
  comparten una funcion testeable.
- **Una sola definicion de "version prerelease"** (`update::is_prerelease`, reusada por el dedup de
  duplicados) y **un solo escaneo del area gestionada** (`modlist::folders_with_declared_id`, reusado
  por `manager` y por la limpieza de duplicados de la sync) — el mismo criterio de seguridad en un lugar.
- **Modulo `util`** con `human_size` (antes duplicado GUI/CLI) y `unique_nanos` (antes 3 copias).
- **`transport::get_text` redacta la URL en los errores** por si un futuro llamador le pasa una URL
  firmada (defensa en profundidad; los caminos con secretos ya lo hacian).

## [1.21.0] - 2026-06-16 — fixes de robustez de la sync

- **Fix (Windows): la sync ya no manda a la papelera un archivo del set que solo difiere en
  MAYUSCULAS.** El FS de Windows es case-insensitive: si tu copia local era `Mod/BaseLib.pck` pero el
  manifest lo declara `Mod/baselib.pck`, el barrido de huerfanos lo veia como "sobrante" y lo
  trasheaba (rompiendo el mod, aunque era recuperable). Ahora la comparacion es case-insensitive en
  Windows (`sync::orphan_key`).
- **Fix: al DESUSCRIBirte de un set tambien se limpia su baseline de version.** Antes quedaba colgado
  en `config.set_versions`/memoria y, al re-suscribirte, resucitaba un "version nueva" viejo.
- **Robustez: `transport::download_capped` se auto-limpia** — si la descarga se corta a la mitad o
  supera el tope de tamaño, borra el archivo parcial en vez de dejarlo (los llamadores ya lo hacian;
  ahora la funcion es self-cleaning para que un futuro llamador no filtre un `.part`).

## [1.20.0] - 2026-06-16 — confirmar antes de pisar un mod por `nxm://` + versiones estable/beta

- **El flujo `nxm://` (boton "Mod Manager Download" de Nexus) ahora CONFIRMA antes de reemplazar** un
  mod que ya tenes instalado (dialogo Si/No; la version vieja va a la papelera, reversible). Como lo
  lanza el protocolo —no la app— antes pisaba el mod en silencio. Si no lo tenes, instala sin
  preguntar. Si el dialogo no se puede mostrar, NO reemplaza (conservador). Nuevos:
  `manager::is_id_installed` + `manager::install_from_zip_confirmed` (extrae una sola vez).
- **Comparacion de versiones: el ESTABLE le gana a su propia beta.** A igual `X.Y.Z`, ahora `1.2.0` se
  considera mas nuevo que `1.2.0-rc1` (asi quien quedo en una beta recibe el estable cuando sale), pero
  una beta no "actualiza" sobre el estable. Afecta el auto-update de mods y la deteccion de set nuevo.

## [1.19.0] - 2026-06-16 — la sync limpia copias duplicadas del mismo mod

- **Tras sincronizar, las carpetas DUPLICADAS de un mod del set se mandan a la papelera** (reversible).
  Si un amigo ya tenia un mod en una carpeta con OTRO nombre (p.ej. `SuperMod-v2/` en vez de la
  canonica `SuperMod/`), o una copia vieja en `mods_disabled/`, la sync la limpia — antes quedaban DOS
  copias del mismo mod cargando a la vez, lo que cambia el room-hash de multiplayer y dejaba al amigo
  afuera del lobby. Solo limpia si la copia canonica `mods/<id>/` ya existe (nunca borra la unica
  copia), nunca toca mods fuera del set (`managed_ids`), y avisa cuantas mando a la papelera. El
  auto-update de un mod ya hacia esto al reinstalar (1.14.1); ahora la sync tambien.

## [1.18.0] - 2026-06-16 — ver QUE cambia un codigo antes de aplicarlo

- **Preview del codigo compartido** (pestaña Perfiles): pegar un codigo y tocar **"Revisar codigo"**
  ya no aplica de una — muestra el IMPACTO ("activa N, desactiva M, ya estan K · faltan J no
  instalados", y lista cuales no tenes) y recien **"Confirmar"** lo aplica. Asi el que recibe un
  codigo de un amigo ve que mods se van a activar/desactivar antes de tocar nada. Revisar es
  solo-lectura (anda con el juego abierto); Confirmar exige el juego cerrado.
- Refactor interno: `profile::apply` ahora se calcula con `profile::preview_from` (puro, sin tocar
  disco) y despues ejecuta — mismo resultado, pero el preview reusa exactamente esa logica.

## [1.17.0] - 2026-06-15 — buscar actualizaciones de TODOS los mods de una

- **"Buscar actualizaciones de todos los mods"** (pestaña Mods): un boton revisa el upstream de cada
  mod con origen conocido (GitHub/Nexus) en un worker y marca los que tienen version nueva con un
  **● update** en la lista; el detalle de cada uno sigue teniendo "Actualizar". Reporta cuantos no se
  pudieron chequear (rate-limit anonimo de GitHub 60/h — recomienda `github-login` para 5000/h). Los
  de Nexus se saltean si no hay API key conectada (no cuentan como fallo). (CLI: `mod-check` ya lo hacia.)
- Polish: el campo del codigo compartible (pestaña Perfiles) es de SOLO LECTURA seleccionable — ya no
  se puede editar/corromper sin querer.

## [1.16.0] - 2026-06-15 — instalar mods de Nexus en `.7z` (no solo `.zip`)

- **Los mods de Nexus en `.7z` ahora se instalan directo** (antes se guardaban a Descargas para
  extraer a mano). Aplica al handler `nxm://` y a la actualizacion directa de Premium. Nuevo:
  `manager::archive_kind` detecta el formato por los bytes MAGIC (no por la extension, que el CDN de
  Nexus a veces omite) y `manager::install_from_zip`/`install_update_zip` extraen `.zip` O `.7z` (via
  `sevenz-rust`, pure-Rust) con la MISMA defensa anti path-traversal que el zip (valida cada entrada
  por componentes). El `.rar` y otros formatos siguen yendo a Descargas (sin extraer).
- Dep nueva: `sevenz-rust` (descompresor 7z pure-Rust, sin C).

## [1.15.0] - 2026-06-15 — compartir la lista de activados/desactivados por CODIGO

- **Compartir tu lista de mods activados (y desactivados) con un codigo.** En la pestaña Perfiles, el
  boton **"Generar codigo de la lista actual"** crea un codigo (`STS2L1...`) que copia al portapapeles;
  cada perfil guardado tiene tambien un boton **"Compartir"**. Un amigo que YA tenga los mods lo pega
  en **"Pegar un codigo" → "Aplicar codigo"**: la app ACTIVA esos mods y DESACTIVA el resto. El orden
  de carga canonico (BaseLib + A-Z) sale solo, asi entran al mismo lobby. NO baja archivos (para eso
  esta la sync) — solo comparte el estado on/off, sin servidor.
- Nuevo modulo `loadcode`: `STS2L1.` + base64url(deflate(JSON)) — autocontenido, tolera que un chat
  corte la linea, y al decodificar UNTRUSTED filtra ids no-simples y CAPA la descompresion (anti
  zip-bomb). Aplica reusando `profile::apply`. CLI: `loadcode` (imprime el codigo) / `loadcode <codigo>`
  (lo aplica). Deps nuevas: `flate2` (pure-Rust, miniz_oxide) + `base64`.

## [1.14.1] - 2026-06-15 — fix: actualizar/reinstalar un mod ya NO deja la version vieja

- **Al instalar/actualizar un mod se quitan TODAS sus copias viejas, no solo `mods/<id>`.** Antes
  `prepare_dst` solo reemplazaba la carpeta llamada EXACTO como el id; si la version vieja estaba en
  una carpeta con otro nombre (tipico al recibir mods por Drive, p.ej. `mods/FGOCore-1.0`) o como una
  copia deshabilitada, quedaba como DUPLICADO. Ahora se buscan todas las carpetas (en `mods/` y
  `mods_disabled/`, cualquier nombre) cuyo `<id>.json` declare ese id y se mandan a la papelera antes
  de copiar la nueva (`manager::dirs_with_id`). Asi el auto-update y la reinstalacion no dejan la
  version anterior. Sigue exigiendo el juego cerrado, nunca toca la fuente, y es reversible (papelera).
- Limite conocido: una carpeta con `<id>.json` ILEGIBLE y nombre != id no se puede atribuir a un id, asi
  que no se detecta (no se infiere por nombre para no borrar un mod equivocado). El boton "Quitar
  duplicados" tampoco la ve. Es un caso raro (manifiesto corrupto).

## [1.14.0] - 2026-06-15 — limpiar mods duplicados (deja la version mas nueva)

- **Quitar mods duplicados de una.** Cuando el mismo `id` esta instalado en mas de una carpeta
  (conflicto que el juego marca), aparece en la pestaña Mods un boton **"Quitar N duplicado(s) — deja
  la version mas nueva (papelera)"**: por cada id conserva la version mas alta y manda las otras a la
  papelera (reversible). CLI: `dedupe`. Nuevo: `modlist::duplicates` (elige el que se conserva por
  version; empate -> release antes que pre-release, luego habilitado, luego mtime) y
  `manager::trash_mod_dir` (borra una carpeta ESPECIFICA, validando que sea hija directa de `mods/` o
  `mods_disabled/` y que el ultimo componente no sea `..` — nunca toca nada afuera; exige el juego
  cerrado).
- Borra por PATH (no por id) porque las carpetas duplicadas suelen tener nombres distintos del id
  (`mods/FGOCore` y `mods/FGOCore-1.2`, ambos id `FGOCore`). Siempre conserva al menos una copia.

## [1.13.0] - 2026-06-15 — fix: "Steam初始化失败 / No appID found" al lanzar + opcion "Jugar por Steam"

- **Lanzar el build de Steam ya no falla con "No appID found".** El juego llama a `SteamAPI_Init`, que
  falla cuando el `.exe` se corre DIRECTO (no desde el cliente de Steam). Dos modos (toggle en la barra
  lateral, solo para builds de Steam):
  - **Jugar por Steam (default):** lanza con `steam://rungameid/2868840`, asi Steam abre el juego con
    integracion COMPLETA — overlay, horas, invitaciones. Es el modo recomendado.
  - **Directo:** abre el exe dejando antes un `steam_appid.txt` con el appID (`2868840`) para que
    SteamAPI inicialice contra el Steam que ya corre (sin overlay, pero anda). Idempotente y best-effort.
- Las copias pirata "limpias" (sin la dll de Steamworks) no usan SteamAPI: siempre van directo y no se
  tocan. CLI: `launch` va por Steam salvo `--direct` (o `launch_via_steam=false` en la config).

## [1.12.0] - 2026-06-15 — publicar incremental (solo lo que cambio) + version automatica

- **Publicar ahora SUBE solo los mods que cambiaron.** Antes cada version creaba un release nuevo y
  re-subia TODOS los assets (un `.pck` de 100+ MB se re-subia aunque no cambiara). Ahora los assets
  (content-addressed por BLAKE3) van a UN release acumulativo (`modsync-assets`, marcado prerelease)
  y publicar una version sube **solo los blake3 que falten** ahi — los que no cambiaron no se
  re-suben. El release de la VERSION (el que `/releases/latest` devuelve) lleva solo el manifest
  chico, con su `base_url` apuntando al release de assets. Nuevo: `github::upload_new_assets`
  (incremental), `ASSETS_TAG`/`assets_base_url`, `collect_manifest_files`/`collect_asset_files`. El
  `--base-url` legacy sigue subiendo todo a ese release (`publish::upload_to_release`).
- **La version se autocompleta.** Al abrir Publicar (con un repo recordado), la app resuelve el
  ultimo release del repo y propone la **siguiente** version (`publish::next_version`: incrementa el
  ultimo grupo de digitos, p.ej. `1.2.0`→`1.2.1`, `2026.06.14`→`2026.06.15`), asi no hay que tipearla.
  Es editable. CLI sin cambios (sigue pidiendo `--version`).
- **Los amigos ya bajaban solo lo que cambio** (no es nuevo, se confirma): `sync::plan` compara el
  BLAKE3 de cada archivo y baja unicamente los que faltan o cambiaron (+ el delta intra-`.pck` baja
  solo el diff de un `.pck` cambiado). Con el publish incremental, ahora AMBOS lados son incrementales.

## [1.11.2] - 2026-06-15 — fix: modo claro tenia el area central con FONDO NEGRO

- **El fondo del area central ya no queda negro en modo claro.** El `Frame` del contenido central era
  TRANSPARENTE, asi que dejaba ver la superficie raiz (oscura): todo lo que NO esta en una card (el
  orden de carga, la fila de busqueda) quedaba sobre negro y casi no se leia. Ahora ese `Frame` se
  rellena con `panel_fill` del tema (claro en tema claro, oscuro en oscuro), igual que la barra de
  navegacion — las cards blancas siguen resaltando sobre el fondo claro.
- La linea "Orden de carga (multiplayer): ..." pasa a estar en color **acento (azul)** para que se
  note (es una linea de info larga).

## [1.11.1] - 2026-06-15 — fix: la pestaña Sync se podia salir de la ventana sin scroll

- **El contenido de las pestañas ahora se puede DESLIZAR.** El area central no tenia un scroll, asi
  que en una ventana baja (la minima es 700x480) el contenido de Sync (cargar set + manifest + plan +
  consentimiento + boton "Instalar") se salia por abajo y no habia forma de llegar al boton. Se
  envolvio el contenido de cada pestaña en un `ScrollArea` vertical; ademas, al fijar un ancho
  definido, las etiquetas largas hacen wrap en vez de salirse por el costado.
- **Filas de inputs/listas de Sync mas robustas:** las filas "URL/Repositorio" y la lista de sets
  guardados usan `horizontal_wrapped` (los botones/indicadores bajan de linea si no entran) y los
  campos de texto tienen un ancho que entra en la ventana minima.

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
