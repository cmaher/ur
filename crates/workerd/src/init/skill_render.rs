//! Install-time rendering of skill source text.
//!
//! Two transforms, applied in order:
//! 1. Cross-skill reference rewrite (plugin layout only): references to other
//!    *installed* skills become `<namespace>:<name>` so the agent picks the ur
//!    copy over a same-named user skill.
//! 2. `%UR_SKILL_HOOKS%` placeholder → the worker's skill-hooks directory.
//!
//! Rewriting runs first so the rendered hooks path is never itself rewritten.

use std::collections::HashSet;
use std::path::Path;

use crate::skill_layout::SkillLayout;

/// Placeholder in skill sources for the skill-hooks directory.
pub const SKILL_HOOKS_PLACEHOLDER: &str = "%UR_SKILL_HOOKS%";

/// Render one skill file's text for `layout`.
///
/// `installed` is the full set of skill names being installed; references to
/// anything outside it are left untouched.
pub fn render_skill_text(
    text: &str,
    layout: &SkillLayout,
    installed: &HashSet<&str>,
    skill_hooks: &Path,
) -> String {
    let rewritten = match layout.namespace() {
        Some(namespace) => {
            let slashed = rewrite_slash_refs(text, namespace, installed);
            rewrite_backtick_refs(&slashed, namespace, installed)
        }
        None => text.to_owned(),
    };
    rewritten.replace(SKILL_HOOKS_PLACEHOLDER, &skill_hooks.to_string_lossy())
}

/// Bytes allowed in a skill-name token (`brain:init`, `code-review`, ...).
fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':')
}

/// Bytes that may precede a `/name` slash-command reference.
fn is_slash_boundary(b: u8) -> bool {
    b.is_ascii_whitespace() || matches!(b, b'`' | b'(' | b'"' | b'\'' | b'[' | b'*')
}

/// Rewrite `/name` slash-command references to `/<namespace>:name`.
///
/// A token is a reference only when it is an installed skill name, starts at
/// a word boundary, and is not followed by `/`. A name the same file also uses
/// as a directory (`/name/...`) is treated as a path everywhere in that file
/// (e.g. the `/brain` mount in the `brain` skill), so it is left alone.
/// `%UR_SKILL_HOOKS%/<name>/` hook includes do not count as such a use.
fn rewrite_slash_refs(text: &str, namespace: &str, installed: &HashSet<&str>) -> String {
    let path_probe = text.replace(&format!("{SKILL_HOOKS_PLACEHOLDER}/"), "");
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'/' || (i > 0 && !is_slash_boundary(bytes[i - 1])) {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < bytes.len() && is_token_byte(bytes[end]) {
            end += 1;
        }
        let token = &text[start..end];
        let is_ref = !token.is_empty()
            && bytes.get(end) != Some(&b'/')
            && installed.contains(token)
            && !path_probe.contains(&format!("/{token}/"));
        if is_ref {
            out.push_str(&text[copied..start]);
            out.push_str(namespace);
            out.push(':');
            out.push_str(token);
            copied = end;
        }
        i = end.max(i + 1);
    }
    out.push_str(&text[copied..]);
    out
}

/// Rewrite "`name` skill" prose references to "`<namespace>:name` skill".
fn rewrite_backtick_refs(text: &str, namespace: &str, installed: &HashSet<&str>) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < bytes.len() && is_token_byte(bytes[end]) {
            end += 1;
        }
        let closed = bytes.get(end) == Some(&b'`');
        let token = &text[start..end];
        let followed_by_skill = closed && {
            let after = &text[end + 1..];
            let rest = after.trim_start_matches([' ', '\t']);
            rest.len() < after.len() && rest.starts_with("skill")
        };
        if followed_by_skill && installed.contains(token) {
            out.push_str(&text[copied..start]);
            out.push_str(namespace);
            out.push(':');
            out.push_str(token);
            copied = end;
        }
        // Skip past the closing backtick so it is not mistaken for an opener.
        i = if closed { end + 1 } else { end.max(i + 1) };
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOOKS: &str = "/home/worker/.claude/skill-hooks";

    fn installed() -> HashSet<&'static str> {
        [
            "implement",
            "code-review",
            "brain",
            "brain:init",
            "dispatch",
        ]
        .into_iter()
        .collect()
    }

    fn plugin(text: &str) -> String {
        render_skill_text(
            text,
            &SkillLayout::ur_plugin(),
            &installed(),
            Path::new(HOOKS),
        )
    }

    #[test]
    fn agent_home_only_renders_placeholder() {
        let src =
            "Run /code-review then the `implement` skill.\n@%UR_SKILL_HOOKS%/implement/a.md\n";
        let out = render_skill_text(src, &SkillLayout::AgentHome, &installed(), Path::new(HOOKS));
        assert_eq!(
            out,
            "Run /code-review then the `implement` skill.\n@/home/worker/.claude/skill-hooks/implement/a.md\n"
        );
    }

    #[test]
    fn plugin_rewrites_slash_refs_to_installed_skills() {
        assert_eq!(plugin("Run /code-review now."), "Run /ur:code-review now.");
        assert_eq!(plugin("`/dispatch` knows"), "`/ur:dispatch` knows");
        assert_eq!(plugin("/implement ur-x"), "/ur:implement ur-x");
        assert_eq!(plugin("(see /implement)"), "(see /ur:implement)");
    }

    #[test]
    fn plugin_leaves_non_installed_and_builtin_refs() {
        assert_eq!(plugin("Run /ship and /clear."), "Run /ship and /clear.");
        assert_eq!(plugin("Use the `ship` skill."), "Use the `ship` skill.");
    }

    #[test]
    fn plugin_leaves_paths_alone() {
        assert_eq!(
            plugin("see ~/.claude/skills/implement/SKILL.md"),
            "see ~/.claude/skills/implement/SKILL.md"
        );
        assert_eq!(plugin("read /implement/x.md"), "read /implement/x.md");
    }

    #[test]
    fn plugin_treats_directory_names_as_paths_file_wide() {
        let src = "mounted at `/brain`. Read `/brain/CLAUDE.md`. Run `/brain:init`.";
        assert_eq!(
            plugin(src),
            "mounted at `/brain`. Read `/brain/CLAUDE.md`. Run `/ur:brain:init`."
        );
    }

    #[test]
    fn plugin_hook_include_does_not_suppress_slash_ref() {
        assert_eq!(
            plugin("@%UR_SKILL_HOOKS%/implement/a.md\nThen run /implement ur-y."),
            "@/home/worker/.claude/skill-hooks/implement/a.md\nThen run /ur:implement ur-y."
        );
    }

    #[test]
    fn plugin_rewrites_backtick_skill_prose() {
        assert_eq!(
            plugin("use the `implement` skill and the `code-review`  skills"),
            "use the `ur:implement` skill and the `ur:code-review`  skills"
        );
        // Not followed by "skill": a plain identifier, untouched.
        assert_eq!(
            plugin("strategy `dispatch` that"),
            "strategy `dispatch` that"
        );
    }

    #[test]
    fn plugin_rewrite_is_idempotent() {
        let once = plugin("Run /code-review and the `implement` skill.");
        assert_eq!(plugin(&once), once);
    }

    #[test]
    fn plugin_never_rewrites_rendered_hook_path() {
        let out = render_skill_text(
            "@%UR_SKILL_HOOKS%/implement/a.md",
            &SkillLayout::ur_plugin(),
            &installed(),
            Path::new("/d/.ur/implement"),
        );
        assert_eq!(out, "@/d/.ur/implement/implement/a.md");
    }

    #[test]
    fn no_placeholder_survives() {
        let src = "%UR_SKILL_HOOKS% a %UR_SKILL_HOOKS%/b";
        for layout in [SkillLayout::AgentHome, SkillLayout::ur_plugin()] {
            let out = render_skill_text(src, &layout, &installed(), Path::new(HOOKS));
            assert!(!out.contains(SKILL_HOOKS_PLACEHOLDER), "{out}");
        }
    }
}
