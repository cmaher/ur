//! The dream config: the first ```` ```toml ```` block in `<brain>/DREAM.md`.
//!
//! Every project-specific fact `ur dream` uses lives here. The block is validated on load so a
//! typo or an unfillable template fails loudly instead of silently turning refs `unknown`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};

static BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?ms)^```toml[^\n]*\n(.*?)^```").unwrap());
static PLACEHOLDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap());

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    #[serde(default)]
    pub off_limits: Vec<String>,
    #[serde(default)]
    pub folders: Vec<Folder>,
    #[serde(default)]
    pub ref_kinds: Vec<RawRefKind>,
    #[serde(default)]
    pub unclassified: RawUnclassified,
    #[serde(default)]
    pub discovery: Discovery,
    #[serde(default)]
    pub thresholds: Thresholds,
    #[serde(default)]
    pub review: Review,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Folder {
    pub path: String,
    pub role: Role,
    #[serde(default)]
    pub promote_to: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Durable,
    Scratch,
    Episodic,
    Procedure,
    Orientation,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawRefKind {
    pub kind: String,
    /// Canonical id template over pattern groups, e.g. `{repo}#{n}`. Unset: the matched text.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub patterns: Vec<RawPattern>,
    /// Read-only check command as argv, with `{group}` placeholders. Unset: uncheckable.
    #[serde(default)]
    pub check: Option<Vec<String>>,
    /// Regex over the check's stdout; group 1 (or the whole match) is the state. Unset: the
    /// first line of stdout.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub done: Vec<String>,
    #[serde(default)]
    pub not_done: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RawPattern {
    Plain(String),
    Full {
        regex: String,
        /// Fixed group values for matches of this pattern, e.g. a default repo.
        #[serde(default)]
        set: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawUnclassified {
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    #[serde(default)]
    pub allowed: bool,
    #[serde(default)]
    pub max_calls: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    #[serde(default = "default_quiet_days")]
    pub quiet_days: i64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            quiet_days: default_quiet_days(),
        }
    }
}

fn default_quiet_days() -> i64 {
    14
}

/// Durable-doc review scheduling. Each doc has its own interval: doubled when a review finds it
/// unchanged, halved when it needed changes, clamped to `[min_days, max_days]`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    #[serde(default = "default_per_run")]
    pub per_run: usize,
    #[serde(default = "default_initial_days")]
    pub initial_days: i64,
    #[serde(default = "default_min_days")]
    pub min_days: i64,
    #[serde(default = "default_max_days")]
    pub max_days: i64,
}

impl Default for Review {
    fn default() -> Self {
        Self {
            per_run: default_per_run(),
            initial_days: default_initial_days(),
            min_days: default_min_days(),
            max_days: default_max_days(),
        }
    }
}

fn default_per_run() -> usize {
    1
}
fn default_initial_days() -> i64 {
    30
}
fn default_min_days() -> i64 {
    7
}
fn default_max_days() -> i64 {
    180
}

/// A validated config with compiled patterns.
#[derive(Debug)]
pub struct Config {
    pub raw: RawConfig,
    pub kinds: Vec<RefKind>,
    pub unclassified: Option<Regex>,
    off_limits: Vec<glob::Pattern>,
}

#[derive(Debug)]
pub struct RefKind {
    pub kind: String,
    pub id: Option<String>,
    pub patterns: Vec<Pattern>,
    pub check: Option<Vec<String>>,
    pub state: Option<String>,
    pub done: Vec<String>,
    pub not_done: Vec<String>,
}

#[derive(Debug)]
pub struct Pattern {
    pub regex: Regex,
    pub set: BTreeMap<String, String>,
}

impl Config {
    pub fn load(brain: &Path) -> Result<Self> {
        let path = brain.join("DREAM.md");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("{} not found: run brain:dream-init", path.display()))?;
        Self::parse(&text).with_context(|| format!("{}", path.display()))
    }

    pub fn parse(dream_md: &str) -> Result<Self> {
        let Some(block) = BLOCK_RE.captures(dream_md) else {
            bail!("no ```toml config block: run brain:dream-init");
        };
        let raw: RawConfig = toml::from_str(&block[1]).context("config block is not valid")?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawConfig) -> Result<Self> {
        let r = &raw.review;
        if !(r.min_days <= r.initial_days && r.initial_days <= r.max_days && r.min_days > 0) {
            bail!("review: need 0 < min_days <= initial_days <= max_days");
        }
        let kinds = raw
            .ref_kinds
            .iter()
            .map(compile_kind)
            .collect::<Result<Vec<_>>>()?;
        let unclassified = raw
            .unclassified
            .pattern
            .as_deref()
            .map(Regex::new)
            .transpose()
            .context("unclassified.pattern")?;
        let off_limits = raw
            .off_limits
            .iter()
            .map(|p| glob::Pattern::new(p).with_context(|| format!("off_limits: {p}")))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            raw,
            kinds,
            unclassified,
            off_limits,
        })
    }

    pub fn kind(&self, name: &str) -> Option<&RefKind> {
        self.kinds.iter().find(|k| k.kind == name)
    }

    pub fn is_off_limits(&self, rel: &str) -> bool {
        self.off_limits.iter().any(|p| p.matches(rel))
    }

    /// Folder paths with the given role, without surrounding slashes.
    pub fn folders(&self, role: Role) -> Vec<String> {
        self.raw
            .folders
            .iter()
            .filter(|f| f.role == role)
            .map(|f| f.path.trim_matches('/').to_string())
            .collect()
    }

    pub fn ignore_unclassified(&self, text: &str) -> bool {
        self.raw
            .unclassified
            .ignore
            .iter()
            .any(|p| text.starts_with(p.as_str()))
    }
}

fn compile_kind(raw: &RawRefKind) -> Result<RefKind> {
    let name = &raw.kind;
    let mut patterns = Vec::new();
    for p in &raw.patterns {
        let (regex, set) = match p {
            RawPattern::Plain(r) => (r.clone(), BTreeMap::new()),
            RawPattern::Full { regex, set } => (regex.clone(), set.clone()),
        };
        let regex = Regex::new(&regex).with_context(|| format!("ref kind {name}: bad pattern"))?;
        let available: BTreeSet<String> = regex
            .capture_names()
            .flatten()
            .map(str::to_string)
            .chain(set.keys().cloned())
            .collect();
        let templates = raw.id.iter().chain(raw.check.iter().flatten());
        let missing = templates
            .flat_map(|t| placeholders(t).map(move |n| (t, n)))
            .find(|(_, n)| !available.contains(*n));
        if let Some((t, n)) = missing {
            bail!(
                "ref kind {name}: pattern {:?} does not provide {{{n}}} used in {t:?}",
                regex.as_str()
            );
        }
        patterns.push(Pattern { regex, set });
    }
    if raw.check.is_some() && raw.done.is_empty() && raw.not_done.is_empty() {
        bail!("ref kind {name}: a check needs `done` or `not_done` states");
    }
    if let Some(s) = &raw.state {
        // Placeholders become escaped group values at check time; any sample value will do.
        let sample: BTreeMap<String, String> = placeholders(s)
            .map(|n| (n.to_string(), "x".to_string()))
            .collect();
        Regex::new(&fill_escaped(s, &sample))
            .with_context(|| format!("ref kind {name}: bad state regex"))?;
    }
    Ok(RefKind {
        kind: raw.kind.clone(),
        id: raw.id.clone(),
        patterns,
        check: raw.check.clone(),
        state: raw.state.clone(),
        done: raw.done.clone(),
        not_done: raw.not_done.clone(),
    })
}

fn placeholders(template: &str) -> impl Iterator<Item = &str> {
    PLACEHOLDER_RE
        .captures_iter(template)
        .map(|c| c.get(1).unwrap().as_str())
}

/// Replace `{name}` with its group value. `None` if a placeholder has no value.
pub fn fill(template: &str, groups: &BTreeMap<String, String>) -> Option<String> {
    let mut missing = false;
    let out = PLACEHOLDER_RE.replace_all(template, |c: &regex::Captures| match groups.get(&c[1]) {
        Some(v) => v.clone(),
        None => {
            missing = true;
            String::new()
        }
    });
    (!missing).then(|| out.into_owned())
}

/// Replace `{name}` placeholders that have a group value with the regex-escaped value; leave
/// everything else (including regex quantifiers like `\d{4}`) untouched.
pub fn fill_escaped(template: &str, groups: &BTreeMap<String, String>) -> String {
    PLACEHOLDER_RE
        .replace_all(template, |c: &regex::Captures| match groups.get(&c[1]) {
            Some(v) => regex::escape(v),
            None => c[0].to_string(),
        })
        .into_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const SAMPLE: &str = r#"# Dream config

Prose the scripts ignore.

```toml
off_limits = ["reference/generated/**"]

[[folders]]
path = "reference"
role = "durable"

[[folders]]
path = "working"
role = "scratch"
promote_to = "reference"

[[ref_kinds]]
kind = "ticket"
id = "{key}"
patterns = ['\b(?P<key>EXAMPLE-\d+)\b']
check = ["example-tracker", "show", "{key}"]
state = '^status: (\w+)$'
done = ["closed"]
not_done = ["open", "in_progress"]

[[ref_kinds]]
kind = "pr"
id = "{repo}#{n}"
check = ["example-forge", "pr-state", "example-org/{repo}#{n}"]
done = ["MERGED"]
not_done = ["OPEN"]

[[ref_kinds.patterns]]
regex = '\bexample-org/(?P<repo>[\w-]+)#(?P<n>\d+)'

[[ref_kinds.patterns]]
regex = '\bPR #(?P<n>\d+)'
set = { repo = "example-repo" }

[unclassified]
pattern = '\b[A-Z][A-Z0-9]{1,9}-\d+\b'
ignore = ["ISO"]

[discovery]
allowed = true
max_calls = 20
```
"#;

    #[test]
    fn parses_sample_and_defaults() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(cfg.kinds.len(), 2);
        assert_eq!(cfg.kinds[1].patterns[1].set["repo"], "example-repo");
        assert_eq!(cfg.folders(Role::Scratch), vec!["working"]);
        assert!(cfg.is_off_limits("reference/generated/a/b.sql"));
        assert!(!cfg.is_off_limits("reference/x.md"));
        assert_eq!(cfg.raw.review.per_run, 1);
        assert_eq!(cfg.raw.thresholds.quiet_days, 14);
    }

    #[test]
    fn rejects_missing_block_unknown_field_and_unfillable_template() {
        assert!(Config::parse("# no block").is_err());
        let typo = "```toml\n[[folders]]\npath = \"a\"\nrole = \"durable\"\npromote = \"b\"\n```\n";
        assert!(Config::parse(typo).is_err());
        let unfillable = "```toml\n[[ref_kinds]]\nkind = \"pr\"\nid = \"{repo}#{n}\"\n\
                          patterns = ['#(?P<n>\\d+)']\n```\n";
        let err = Config::parse(unfillable).unwrap_err();
        assert!(format!("{err:#}").contains("{repo}"), "{err:#}");
    }

    #[test]
    fn check_without_states_is_rejected() {
        let cfg = "```toml\n[[ref_kinds]]\nkind = \"t\"\npatterns = ['T-\\d+']\n\
                   check = [\"x\"]\n```\n";
        assert!(Config::parse(cfg).is_err());
    }

    #[test]
    fn fill_substitutes_known_names_only() {
        let g: BTreeMap<String, String> = [("n".into(), "4.2".into())].into();
        assert_eq!(fill("pr/{n}", &g).as_deref(), Some("pr/4.2"));
        assert_eq!(fill("{repo}/{n}", &g), None);
        assert_eq!(fill_escaped(r"^{n}\s+\d{4}$", &g), r"^4\.2\s+\d{4}$");
    }
}
