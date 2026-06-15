# Politica de seguridad — sts2-modsync

`sts2-modsync` baja archivos (`.dll`/`.pck`) que **el juego ejecuta**. Por eso el camino de
descarga/instalacion es la superficie critica y se defiende en capas. Este documento describe el
modelo de confianza y como reportar un problema.

## Modelo de confianza (que garantiza y que NO)

- **Firma minisign — capa OPCIONAL solo para set-manifests.** Para los **set-manifests (sync)** la
  firma es **opcional** (`verify_optional`): el ancla de confianza es que bajaste el manifest por
  HTTPS desde el repo del publicador que un amigo te paso (autenticado por GitHub) y que cada asset
  se verifica por BLAKE3. Si el set viene firmado con la clave del publicador
  (`signing::PUBLISHER_PUBKEY`) se verifica (capa extra) y una firma INVALIDA se **rechaza**
  (tampering); si no, se acepta como "sin firma" y la UI lo muestra.
  TRADE-OFF de un set sin firma: una cuenta de GitHub del publicador comprometida podria servir un
  manifest malicioso (la firma protegia contra eso); por eso firmar sigue siendo recomendado.
- **Binario de auto-update — SIN firma (desde v1.11.0).** El auto-update ya NO exige firma minisign.
  Su ancla es: **HTTPS** + que el release viene del **repo del dueño** (estandar para auto-updaters)
  + un **`--health-check`** del exe nuevo con **rollback al `.bak`** si no arranca, ANTES de relanzar
  (no se brickea la instalacion). TRADE-OFF (residual conocido): el `--health-check` es un control de
  ROBUSTEZ, NO de autenticidad — un exe malicioso lo pasa trivialmente. Por eso, sin firma, una
  **cuenta/release de GitHub comprometida** —o un **TLS-MITM** con un cert mal emitido para el host de
  GitHub— podria servir un `sts2-modsync.exe` arbitrario que la app baja y EJECUTA. La firma minisign
  cerraba ese vector; se removio a pedido (simplicidad: nadie maneja claves). Si se quiere recuperar
  esa garantia, restaurar una firma (aunque sea opcional) o fijar el BLAKE3/SHA-256 del exe por un
  canal de confianza.
- **Hash BLAKE3 por archivo.** Cada `FileEntry` lleva su `blake3`; `sync::apply` verifica cada
  `.part` ANTES de instalarlo. Bajar de un peer P2P no confiable es seguro porque los bytes se
  verifican contra el hash del manifest firmado.
- **HTTPS obligatorio.** Manifest, firma y assets se bajan SIEMPRE por HTTPS
  (`transport::require_https`); `http://` se rechaza. El auto-update tambien exige HTTPS (es su ancla
  principal ahora que no hay firma).
- **Acotado a `managed_ids()`.** El sync solo crea/actualiza/limpia las carpetas `<id>/` listadas
  en el manifest; jamas toca mods ajenos. El `id` se valida (`manifest::validate_ids`) y los
  `files[].path` tambien (`manifest::validate_paths`) contra path-traversal (`..`, separadores,
  rutas absolutas). El install local desde `.zip` se extrae con proteccion anti zip-slip.
- **Apply transaccional.** Todo a `.part` + verificado; recien entonces renames con backup +
  rollback, abortando si el juego corre (lock de `.dll`/`.pck` en Windows).

Lo que la firma **NO** garantiza: la inocuidad del codigo del mod. La firma prueba AUTENTICIDAD e
INTEGRIDAD (viene del publicador y no fue alterado) — el usuario sigue confiando en el publicador.

### Limitaciones conocidas

- **Una sola pubkey empotrada (TOFU).** Hoy hay un unico publicador de confianza; no escala a
  "mi amigo tambien publica" sin recompilar. Rotacion/multi-publisher: pendiente (ver ROADMAP).
- **Modo dev.** Con `PUBLISHER_PUBKEY` vacia la firma NO se verifica; la UI lo muestra en rojo.
  No usar sets de terceros en modo dev.
- **SmartScreen.** El binario no esta firmado con Authenticode (pago), asi que Windows puede
  marcar "editor desconocido". Desde v1.11.0 el zip del release tampoco lleva `.minisig` (el
  auto-update no verifica firma; ver el modelo de confianza arriba).

## Reportar una vulnerabilidad

Abri un issue **privado** (Security advisory) en
`https://github.com/YX14ng/sts2-modsync/security/advisories` o contactá al dueño del repo. No
publiques un exploit hasta que haya un arreglo. Incluí: version afectada, pasos para reproducir y
el impacto (especialmente si permite ejecutar/escribir codigo fuera de `mods/`).
