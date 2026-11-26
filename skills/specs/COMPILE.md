# Compiling Skill Specs

How to compile `skills/specs/<name>.md` into target `SKILL.md` files.

## Inputs & Outputs
- **Spec:** `skills/specs/<name>.md`
- **Profiles:** `skills/specs/_profiles/worker.md` and `skills/specs/_profiles/local.md`
- **Outputs** (per `Targets:` line in the spec):
  - `worker` → `containers/worker-base/potential-skills/<name>/SKILL.md`
  - `local` → `skills/local/<name>/SKILL.md`
- **Assets:** if the spec lists `Assets:`, preserve/copy those supporting files into each target's skill directory alongside `SKILL.md`.

## Authoring Rules
1. **Frontmatter:** YAML with ONLY `name` and `description` (≤ 1024 chars total).
   - `description` starts with `"Use when ..."` in third person, listing concrete triggering symptoms/contexts.
   - **Never summarize the skill's workflow in `description`** (prevents agents from shortcutting the skill body).
2. **Zero target leakage:**
   - `local` outputs must NEVER contain `ur-workertools`, `@%UR_SKILL_HOOKS%`, `step-complete`, `request-human`, or unattended-worker boilerplate.
   - `worker` outputs must fold in all relevant rules from `_profiles/worker.md` cleanly and naturally.
   - If a skill has no `## Worker` or `## Local` differences, compile once and write identical content (with target-specific provenance footer) to both outputs.
3. **Preserve every invariant:** every bullet in `## Core`, the target section, and `## Guardrails` must appear in the compiled skill as concrete, unambiguous instructions. Drop `<!-- REVIEW: ... -->` notes from compiled outputs.
4. **Tight & scannable:** prefer numbered steps, short tables, and explicit red-flag lists over narrative prose. Keep word counts lean while keeping discipline rules bulletproof against rationalization.
5. **Preserve `$ARGUMENTS`** at the bottom only when the skill parses invocation arguments.
6. **Provenance footer:** end every compiled `SKILL.md` with:
   ```html
   <!-- compiled-from: skills/specs/<name>.md@<spec-sha12> profile:<target>@<profile-sha12> -->
   ```
   where `<sha12>` is the first 12 hex chars of `shasum -a 256` of the respective file.

## Verification After Compiling
- Run `scripts/check-skills.sh` (verifies all hashes match, no missing targets, no forbidden `ur-workertools` / `@%UR_SKILL_HOOKS%` leaks in `skills/local/`).
- Run `scripts/link-skills.sh` to refresh home symlinks.
