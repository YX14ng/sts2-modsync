//! Protocolo `nxm://` (fase 2b): registrar la app como handler del boton "Mod Manager Download" de
//! Nexus y parsear el link que la web le pasa. Cuando el usuario toca ese boton en la pagina de un
//! mod, el navegador invoca `nxm://<game>/mods/<mod_id>/files/<file_id>?key=...&expires=...` y Windows
//! se lo entrega a esta app (registrada en HKCU). La CLI `nxm <link>` resuelve el download-link via la
//! API de Nexus (`nexus::download_link`, con el `key`/`expires` que viene en el link para usuarios
//! gratis) y baja+instala. El `key`/`expires` son de un solo uso y caducan: por eso el flujo lo INICIA
//! la web, no la app.
//!
//! Registrar `nxm://` lo TOMA de Vortex/Mod Organizer si los tenes: es opt-in (un boton) y reversible.

use anyhow::Result;

/// Un link `nxm://` parseado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NxmLink {
    pub game: String,
    pub mod_id: u64,
    pub file_id: u64,
    /// `key`/`expires` de un solo uso que genera la web para usuarios gratis (None si Premium directo).
    pub key: Option<String>,
    pub expires: Option<String>,
}

/// Parsea `nxm://<game>/mods/<mod_id>/files/<file_id>[?key=..&expires=..&...]`. `None` si no matchea.
pub fn parse_nxm_link(link: &str) -> Option<NxmLink> {
    let rest = link.trim().strip_prefix("nxm://")?;
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    // game / "mods" / mod_id / "files" / file_id
    if parts.len() < 5 || parts[1] != "mods" || parts[3] != "files" {
        return None;
    }
    let game = parts[0];
    if game.is_empty() || !game.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let mod_id: u64 = parts[2].parse().ok()?;
    let file_id: u64 = parts[4].parse().ok()?;
    let mut key = None;
    let mut expires = None;
    if let Some(q) = query {
        for kv in q.split('&') {
            if let Some((k, v)) = kv.split_once('=') {
                match k {
                    "key" => key = Some(v.to_string()),
                    "expires" => expires = Some(v.to_string()),
                    _ => {}
                }
            }
        }
    }
    Some(NxmLink {
        game: game.to_string(),
        mod_id,
        file_id,
        key,
        expires,
    })
}

const NXM_KEY: &str = r"Software\Classes\nxm";
const SHELL_KEY: &str = r"Software\Classes\nxm\shell";
const CMD_KEY: &str = r"Software\Classes\nxm\shell\open\command";
/// Valor donde guardamos el handler de `nxm://` que habia ANTES (Vortex/MO2), para restaurarlo al
/// desregistrar y NO dejar al usuario sin su gestor de mods.
const PREV_VALUE: &str = "Sts2ModsyncPrevHandler";

/// Lee el comando del handler `nxm://` actual (la default de `shell\open\command`), si hay.
#[cfg(windows)]
fn read_command() -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(CMD_KEY)
        .ok()?
        .get_value::<String, _>("")
        .ok()
}

/// Lee el comando del handler como valor CRUDO (preserva el tipo: REG_SZ vs REG_EXPAND_SZ). Para
/// respaldar el handler previo y restaurarlo TAL CUAL (si usara `%VAR%`, el tipo importa).
#[cfg(windows)]
fn read_command_raw() -> Option<winreg::RegValue> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(CMD_KEY)
        .ok()?
        .get_raw_value("")
        .ok()
}

/// El ejecutable (argv[0]) de un comando del registro: el texto entre las PRIMERAS comillas dobles,
/// o el primer token si no estuviera entre comillas. Asi `command_is_ours` compara la RUTA del
/// handler, no un substring de toda la linea (un comando AJENO que use NUESTRA ruta como ARGUMENTO
/// no debe matchear).
#[cfg(windows)]
fn command_exe(cmd: &str) -> Option<&str> {
    let cmd = cmd.trim();
    match cmd.strip_prefix('"') {
        Some(rest) => rest.split('"').next(),
        None => cmd.split_whitespace().next(),
    }
}

/// `true` si `cmd` lanza el exe ACTUAL: compara la RUTA de argv[0] (no un substring del nombre ni de
/// toda la linea). Sin falso negativo si el exe se renombro (current_exe refleja el nombre nuevo),
/// sin falso positivo si la carpeta contiene el nombre o si otro comando usa nuestra ruta como argumento.
#[cfg(windows)]
fn command_is_ours(cmd: &str) -> bool {
    let Some(exe_in_cmd) = command_exe(cmd) else {
        return false;
    };
    std::env::current_exe()
        .ok()
        .is_some_and(|exe| exe_in_cmd.eq_ignore_ascii_case(&exe.display().to_string()))
}

/// Registra esta app como handler de `nxm://` en HKCU (per-user, sin admin). Si habia OTRO handler
/// (Vortex/MO2), lo RESPALDA para restaurarlo al desregistrar. Es opt-in (un boton).
#[cfg(windows)]
pub fn register() -> Result<()> {
    use anyhow::Context;
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let exe = std::env::current_exe().context("no se pudo resolver el exe actual")?;
    let cmd = format!("\"{}\" nxm \"%1\"", exe.display());
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (nxm, _) = hkcu
        .create_subkey(NXM_KEY)
        .context("creando la clave nxm en el registro")?;
    // Respaldar el handler previo (Vortex/MO2) si lo hay y NO es nuestro (no pisar nuestro propio
    // backup). Se guarda el valor CRUDO (preserva el tipo REG_EXPAND_SZ) para restaurarlo intacto.
    if let Some(prev) = read_command()
        && !command_is_ours(&prev)
        && let Some(raw) = read_command_raw()
    {
        let _ = nxm.set_raw_value(PREV_VALUE, &raw);
    }
    nxm.set_value("", &"URL:NXM Protocol")
        .context("escribiendo el registro")?;
    nxm.set_value("URL Protocol", &"")
        .context("escribiendo el registro")?;
    let (cmdkey, _) = hkcu
        .create_subkey(CMD_KEY)
        .context("creando shell\\open\\command")?;
    cmdkey
        .set_value("", &cmd)
        .context("escribiendo el comando del handler")?;
    Ok(())
}

/// Saca NUESTRO handler de `nxm://`: si habiamos respaldado uno previo (Vortex/MO2) lo RESTAURA, sino
/// borra la clave. Si el handler actual NO es nuestro (alguien lo reemplazo), no toca nada.
#[cfg(windows)]
pub fn unregister() -> Result<()> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    // Si el handler actual no es nuestro, respetarlo (no clobberear el de otra app).
    if read_command()
        .as_deref()
        .is_some_and(|c| !command_is_ours(c))
    {
        return Ok(());
    }
    // ¿Habiamos respaldado un handler previo (Vortex/MO2)? Restaurarlo TAL CUAL (raw -> preserva el
    // tipo) y limpiar el respaldo.
    let prev = hkcu
        .open_subkey(NXM_KEY)
        .ok()
        .and_then(|k| k.get_raw_value(PREV_VALUE).ok());
    if let Some(prev) = prev {
        if let Ok((cmdkey, _)) = hkcu.create_subkey(CMD_KEY) {
            let _ = cmdkey.set_raw_value("", &prev);
        }
        if let Ok((nxm, _)) = hkcu.create_subkey(NXM_KEY) {
            let _ = nxm.delete_value(PREV_VALUE);
        }
        return Ok(());
    }
    // Eramos el unico handler: sacar SOLO lo que escribimos (el subarbol `shell` + nuestros dos
    // valores), NO un wipe recursivo de toda la clave `nxm` (otra app podria convivir con datos ahi).
    // La clave `nxm` se borra solo si quedo VACIA.
    let _ = hkcu.delete_subkey_all(SHELL_KEY);
    if let Ok(nxm) = hkcu.open_subkey_with_flags(NXM_KEY, KEY_ALL_ACCESS) {
        let _ = nxm.delete_value(""); // "URL:NXM Protocol"
        let _ = nxm.delete_value("URL Protocol");
        let empty = nxm.enum_keys().next().is_none() && nxm.enum_values().next().is_none();
        drop(nxm);
        if empty {
            let _ = hkcu.delete_subkey(NXM_KEY);
        }
    }
    Ok(())
}

/// `true` si ESTA app (su exe actual) es el handler de `nxm://`.
#[cfg(windows)]
pub fn is_registered() -> bool {
    read_command().as_deref().is_some_and(command_is_ours)
}

#[cfg(not(windows))]
pub fn register() -> Result<()> {
    anyhow::bail!("el handler nxm:// solo esta soportado en Windows")
}
#[cfg(not(windows))]
pub fn unregister() -> Result<()> {
    Ok(())
}
#[cfg(not(windows))]
pub fn is_registered() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nxm_link_ok_y_rechazo() {
        let l = parse_nxm_link(
            "nxm://slaythespire/mods/266/files/1234?key=ABC123&expires=1700000000&user_id=42",
        )
        .unwrap();
        assert_eq!(l.game, "slaythespire");
        assert_eq!(l.mod_id, 266);
        assert_eq!(l.file_id, 1234);
        assert_eq!(l.key.as_deref(), Some("ABC123"));
        assert_eq!(l.expires.as_deref(), Some("1700000000"));
        // sin query (Premium directo): key/expires None.
        let l2 = parse_nxm_link("nxm://slaythespire/mods/1/files/2").unwrap();
        assert_eq!((l2.mod_id, l2.file_id), (1, 2));
        assert!(l2.key.is_none());
        // rechazos.
        assert!(parse_nxm_link("https://www.nexusmods.com/slaythespire/mods/266").is_none());
        assert!(parse_nxm_link("nxm://slaythespire/mods/266").is_none()); // sin files/<id>
        assert!(parse_nxm_link("nxm://slaythespire/mods/abc/files/1").is_none()); // mod_id no num
        assert!(parse_nxm_link("nxm://ev il/mods/1/files/2").is_none()); // game con espacio
    }

    #[cfg(windows)]
    #[test]
    fn command_exe_extrae_argv0_no_un_substring() {
        // Nuestro comando: argv[0] es el exe entre comillas.
        assert_eq!(
            command_exe(r#""C:\Tools\sts2-modsync.exe" nxm "%1""#),
            Some(r"C:\Tools\sts2-modsync.exe")
        );
        // Sin comillas: el primer token.
        assert_eq!(
            command_exe(r"C:\Tools\app.exe %1"),
            Some(r"C:\Tools\app.exe")
        );
        // Comando AJENO que usa NUESTRA ruta como ARGUMENTO: argv[0] es el launcher, NO nuestra ruta
        // (asi `command_is_ours` no da falso positivo y `unregister` no clobberea ese handler).
        assert_eq!(
            command_exe(r#""C:\Other\launcher.exe" --run "C:\Tools\sts2-modsync.exe""#),
            Some(r"C:\Other\launcher.exe")
        );
    }

    #[cfg(windows)]
    #[test]
    fn registrar_y_desregistrar_round_trip() {
        // Muta HKCU\Software\Classes\nxm. Solo corre si nxm:// NO estaba registrado de antes (para no
        // pisar un handler real de Vortex/MO2 del dev). `is_registered` compara la RUTA de argv[0] con
        // current_exe(), asi que funciona aun corriendo desde el binario de test ("sts2_modsync-<hash>").
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if hkcu.open_subkey(NXM_KEY).is_ok() {
            eprintln!("(skip: ya hay un handler nxm:// registrado; no lo tocamos)");
            return;
        }
        register().unwrap();
        assert!(
            is_registered(),
            "is_registered deberia matchear nuestro exe"
        );
        let cmd = read_command().unwrap();
        assert!(
            cmd.contains("nxm") && cmd.contains("%1"),
            "el comando deberia ser \"<exe>\" nxm \"%1\": {cmd}"
        );
        unregister().unwrap();
        assert!(!is_registered());
        assert!(
            hkcu.open_subkey(NXM_KEY).is_err(),
            "sin handler previo, la clave deberia borrarse"
        );
    }
}
