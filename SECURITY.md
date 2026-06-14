# Politica de seguridad — sts2-modsync

`sts2-modsync` baja archivos (`.dll`/`.pck`) que **el juego ejecuta**. Por eso el camino de
descarga/instalacion es la superficie critica y se defiende en capas. Este documento describe el
modelo de confianza y como reportar un problema.

## Modelo de confianza (que garantiza y que NO)

- **Firma minisign (P0).** Cada set-manifest se firma con la clave PRIVADA del publicador; el
  cliente lleva la clave PUBLICA empotrada (`signing::PUBLISHER_PUBKEY`) y **rechaza** todo
  manifest cuya firma no valide. Cierra "un atacante sustituye el manifest/.dll en el hosting o
  hace MITM". El binario de auto-update se firma y verifica igual ANTES de reemplazar el exe.
- **Hash BLAKE3 por archivo.** Cada `FileEntry` lleva su `blake3`; `sync::apply` verifica cada
  `.part` ANTES de instalarlo. Bajar de un peer P2P no confiable es seguro porque los bytes se
  verifican contra el hash del manifest firmado.
- **HTTPS obligatorio.** Manifest, firma y assets se bajan SIEMPRE por HTTPS
  (`transport::require_https`); `http://` se rechaza. El auto-update tambien exige HTTPS.
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
  marcar "editor desconocido". El zip del release SI lleva su `.minisig` minisign.

## Reportar una vulnerabilidad

Abri un issue **privado** (Security advisory) en
`https://github.com/YX14ng/sts2-modsync/security/advisories` o contactá al dueño del repo. No
publiques un exploit hasta que haya un arreglo. Incluí: version afectada, pasos para reproducir y
el impacto (especialmente si permite ejecutar/escribir codigo fuera de `mods/`).
