use serde::{Deserialize, Serialize};

/// Environment variable name for the resolved forge type.
pub const UR_WORKER_FORGE_ENV: &str = "UR_WORKER_FORGE";

/// Supported forge platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForgeType {
    GitHub,
    Gitea,
}

impl ForgeType {
    /// Return the lowercase string slice for this forge type ("github" or "gitea").
    pub fn as_str(&self) -> &'static str {
        match self {
            ForgeType::GitHub => "github",
            ForgeType::Gitea => "gitea",
        }
    }
}

impl std::fmt::Display for ForgeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Detect the forge type from a repository URL.
///
/// Returns [`ForgeType::GitHub`] if the URL points to a GitHub repository
/// (e.g. host is `"github.com"`), otherwise defaults to [`ForgeType::Gitea`].
pub fn parse_forge_from_url(url: &str) -> ForgeType {
    let lower = url.trim().to_lowercase();
    if is_github_url(&lower) {
        ForgeType::GitHub
    } else {
        ForgeType::Gitea
    }
}

fn is_github_url(lower: &str) -> bool {
    // Handle scheme-based URLs (https://github.com/..., ssh://git@github.com/...)
    if let Some((_, rest)) = lower.split_once("://") {
        let host_part = rest.split(['/', ':', '?']).next().unwrap_or("");
        let host = if let Some((_, h)) = host_part.split_once('@') {
            h
        } else {
            host_part
        };
        return host == "github.com" || host.ends_with(".github.com");
    }
    // Handle SCP-style URLs (git@github.com:org/repo.git)
    if let Some((user_host, _)) = lower.split_once(':') {
        let host = if let Some((_, h)) = user_host.split_once('@') {
            h
        } else {
            user_host
        };
        if host == "github.com" || host.ends_with(".github.com") {
            return true;
        }
    }
    // Handle bare path/domain format (github.com/org/repo)
    let host = lower.split(['/', ':']).next().unwrap_or("");
    host == "github.com" || host.ends_with(".github.com")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_forge_from_url_github() {
        assert_eq!(
            parse_forge_from_url("https://github.com/org/repo.git"),
            ForgeType::GitHub
        );
        assert_eq!(
            parse_forge_from_url("git@github.com:org/repo.git"),
            ForgeType::GitHub
        );
        assert_eq!(
            parse_forge_from_url("HTTPS://GITHUB.COM/ORG/REPO"),
            ForgeType::GitHub
        );
        assert_eq!(
            parse_forge_from_url("ssh://git@github.com/org/repo"),
            ForgeType::GitHub
        );
        assert_eq!(
            parse_forge_from_url("http://github.com/org/repo"),
            ForgeType::GitHub
        );
        assert_eq!(
            parse_forge_from_url("ssh://git@github.com:2222/org/repo.git"),
            ForgeType::GitHub
        );
        assert_eq!(
            parse_forge_from_url("github.com/org/repo"),
            ForgeType::GitHub
        );
    }

    #[test]
    fn test_parse_forge_from_url_gitea() {
        assert_eq!(
            parse_forge_from_url("https://gitea.example.com/org/repo.git"),
            ForgeType::Gitea
        );
        assert_eq!(
            parse_forge_from_url("git@git.mycompany.org:team/project.git"),
            ForgeType::Gitea
        );
        assert_eq!(
            parse_forge_from_url("https://gitlab.com/org/repo.git"),
            ForgeType::Gitea
        );
        assert_eq!(
            parse_forge_from_url("ssh://git@192.168.1.100:2222/org/repo.git"),
            ForgeType::Gitea
        );
        assert_eq!(
            parse_forge_from_url("git@example.com:user/project.git"),
            ForgeType::Gitea
        );
        assert_eq!(
            parse_forge_from_url("https://gitea.example.com/mirrors/github.com/repo.git"),
            ForgeType::Gitea
        );
        assert_eq!(
            parse_forge_from_url("git@gitea.example.com:mirrors/github.com/repo.git"),
            ForgeType::Gitea
        );
        assert_eq!(
            parse_forge_from_url("https://notgithub.com/org/repo.git"),
            ForgeType::Gitea
        );
        assert_eq!(parse_forge_from_url(""), ForgeType::Gitea);
    }

    #[test]
    fn test_forge_type_serde() {
        assert_eq!(
            serde_json::to_string(&ForgeType::GitHub).unwrap(),
            "\"github\""
        );
        assert_eq!(
            serde_json::to_string(&ForgeType::Gitea).unwrap(),
            "\"gitea\""
        );

        let gh: ForgeType = serde_json::from_str("\"github\"").unwrap();
        assert_eq!(gh, ForgeType::GitHub);

        let gitea: ForgeType = serde_json::from_str("\"gitea\"").unwrap();
        assert_eq!(gitea, ForgeType::Gitea);

        let toml_gh: ForgeType = toml::from_str("\"github\"").unwrap();
        assert_eq!(toml_gh, ForgeType::GitHub);

        let toml_gitea: ForgeType = toml::from_str("\"gitea\"").unwrap();
        assert_eq!(toml_gitea, ForgeType::Gitea);
    }

    #[test]
    fn test_forge_type_display_and_as_str() {
        assert_eq!(ForgeType::GitHub.as_str(), "github");
        assert_eq!(ForgeType::Gitea.as_str(), "gitea");
        assert_eq!(ForgeType::GitHub.to_string(), "github");
        assert_eq!(ForgeType::Gitea.to_string(), "gitea");
    }
}
