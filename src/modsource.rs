//! De donde viene un mod (su "upstream") para auto-actualizarlo: un repo de GitHub o una pagina de
//! Nexus Mods. El origen se obtiene del `<id>.json` del mod (campos `url`/`homepage`/`repository`) o
//! lo pega el usuario (se recuerda en `config.mod_sources`). `modupdate` lo usa para chequear/bajar
//! la version nueva. GitHub = auto-update completo (gratis); Nexus = chequeo gratis pero la descarga
//! auto necesita Premium o el handler `nxm://` (fase 2).

use crate::github;

/// El upstream de un mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModSource {
    /// Releases de un repo de GitHub.
    GitHub { owner: String, repo: String },
    /// Pagina de un mod en Nexus (game domain + mod id).
    Nexus { game: String, mod_id: u64 },
}

impl ModSource {
    /// Parsea un link/string a un origen (`None` si no matchea; el charset de owner/repo se valida
    /// con `github::normalize_repo`). Acepta:
    ///  - GitHub: `github:owner/repo`, `https://github.com/owner/repo[/...]`, o `owner/repo` suelto.
    ///  - Nexus: `nexus:game/123` o `https://www.nexusmods.com/<game>/mods/123[?...]`.
    pub fn parse(input: &str) -> Option<ModSource> {
        let s = input.trim();
        if s.is_empty() {
            return None;
        }
        // Nexus explicito (`nexus:game/id`) o por URL.
        if let Some(rest) = s.strip_prefix("nexus:") {
            let (game, id) = rest.split_once('/')?;
            return nexus(game, id);
        }
        if let Some(src) = parse_nexus_url(s) {
            return Some(src);
        }
        // GitHub: explicito, URL, o `owner/repo` suelto -> normalize_repo valida el charset.
        let gh = s.strip_prefix("github:").unwrap_or(s);
        let repo = github::normalize_repo(gh)?;
        let (owner, name) = repo.split_once('/')?;
        Some(ModSource::GitHub {
            owner: owner.to_string(),
            repo: name.to_string(),
        })
    }

    /// Forma canonica para guardar en `config.mod_sources` y re-parsear despues.
    pub fn to_storage(&self) -> String {
        match self {
            ModSource::GitHub { owner, repo } => format!("github:{owner}/{repo}"),
            ModSource::Nexus { game, mod_id } => format!("nexus:{game}/{mod_id}"),
        }
    }

    /// Etiqueta legible para la UI.
    pub fn label(&self) -> String {
        match self {
            ModSource::GitHub { owner, repo } => format!("GitHub · {owner}/{repo}"),
            ModSource::Nexus { game, mod_id } => format!("Nexus · {game}/{mod_id}"),
        }
    }

    /// URL de la pagina web del mod (para el boton "Abrir").
    pub fn web_url(&self) -> String {
        match self {
            ModSource::GitHub { owner, repo } => format!("https://github.com/{owner}/{repo}"),
            ModSource::Nexus { game, mod_id } => {
                format!("https://www.nexusmods.com/{game}/mods/{mod_id}")
            }
        }
    }

    /// `true` si este origen soporta descarga automatica directa hoy (GitHub si; Nexus necesita
    /// Premium o el handler `nxm://`, que es fase 2).
    pub fn supports_auto_download(&self) -> bool {
        matches!(self, ModSource::GitHub { .. })
    }
}

fn nexus(game: &str, id: &str) -> Option<ModSource> {
    let game = game.trim();
    let mod_id: u64 = id.trim().parse().ok()?;
    // El game-domain de Nexus es alfanumerico ASCII (ej "slaythespire", "baldursgate3"); validar el
    // charset (como owner/repo de GitHub) asi un valor raro no arma una URL/`web_url` con basura.
    if game.is_empty() || !game.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(ModSource::Nexus {
        game: game.to_string(),
        mod_id,
    })
}

/// `https://www.nexusmods.com/<game>/mods/<id>[/...|?...]` (o con `games/<game>`). `game` = el
/// segmento INMEDIATAMENTE anterior a `mods`, `id` = el siguiente.
fn parse_nexus_url(s: &str) -> Option<ModSource> {
    let rest = s
        .strip_prefix("https://www.nexusmods.com/")
        .or_else(|| s.strip_prefix("https://nexusmods.com/"))
        .or_else(|| s.strip_prefix("http://www.nexusmods.com/"))
        .or_else(|| s.strip_prefix("www.nexusmods.com/"))?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    let pos = parts.iter().position(|p| *p == "mods")?;
    let game = parts.get(pos.checked_sub(1)?)?;
    let id = parts.get(pos + 1)?;
    nexus(game, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_varias_formas() {
        let want = ModSource::GitHub {
            owner: "YX14ng".into(),
            repo: "sts2-modsync".into(),
        };
        assert_eq!(ModSource::parse("YX14ng/sts2-modsync"), Some(want.clone()));
        assert_eq!(
            ModSource::parse("github:YX14ng/sts2-modsync"),
            Some(want.clone())
        );
        assert_eq!(
            ModSource::parse("https://github.com/YX14ng/sts2-modsync/releases"),
            Some(want.clone())
        );
        // round-trip por to_storage.
        assert_eq!(ModSource::parse(&want.to_storage()), Some(want));
    }

    #[test]
    fn parse_nexus_url_y_explicito() {
        let want = ModSource::Nexus {
            game: "slaythespire".into(),
            mod_id: 266,
        };
        assert_eq!(
            ModSource::parse("https://www.nexusmods.com/slaythespire/mods/266"),
            Some(want.clone())
        );
        assert_eq!(
            ModSource::parse("https://www.nexusmods.com/slaythespire/mods/266?tab=files"),
            Some(want.clone())
        );
        assert_eq!(
            ModSource::parse("nexus:slaythespire/266"),
            Some(want.clone())
        );
        // tambien la forma con games/<game>/mods/<id>.
        assert_eq!(
            ModSource::parse("https://www.nexusmods.com/games/slaythespire/mods/266"),
            Some(want.clone())
        );
        assert_eq!(ModSource::parse(&want.to_storage()), Some(want));
    }

    #[test]
    fn rechaza_basura_y_distingue_capacidades() {
        assert_eq!(ModSource::parse(""), None);
        assert_eq!(ModSource::parse("nexus:slaythespire/noesnumero"), None);
        assert_eq!(ModSource::parse("solounsegmento"), None);
        assert!(
            ModSource::parse("YX14ng/sts2")
                .unwrap()
                .supports_auto_download()
        );
        assert!(
            !ModSource::parse("nexus:slaythespire/1")
                .unwrap()
                .supports_auto_download()
        );
    }
}
