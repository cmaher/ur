#!/usr/bin/env bash
# Validate compiled worker and local skills against skills/specs/.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"

python3 - "$REPO_DIR" <<'PY'
import hashlib
import os
import re
import sys

repo = sys.argv[1]
specs_dir = os.path.join(repo, "skills", "specs")
worker_root = os.path.join(repo, "containers", "worker-base", "potential-skills")
vendor_root = os.path.join(repo, "containers", "worker-base", "vendor", "superpowers", "skills")
local_root = os.path.join(repo, "skills", "local")
strategy_rs = os.path.join(repo, "crates", "server", "src", "strategy.rs")

errors = []


def sha12(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()[:12]


prof_sha = {}
for target in ("worker", "local"):
    p = os.path.join(specs_dir, "_profiles", f"{target}.md")
    if not os.path.isfile(p):
        errors.append(f"missing profile: {p}")
    else:
        prof_sha[target] = sha12(p)

local_leak = re.compile(
    r"ur-workertools|UR_SKILL_HOOKS|step-complete|request-human|pause-nudge|ur host-exec|host policy",
    re.I,
)

expected = {"worker": set(), "local": set()}

spec_files = sorted(
    f
    for f in os.listdir(specs_dir)
    if f.endswith(".md") and f != "COMPILE.md" and os.path.isfile(os.path.join(specs_dir, f))
)

if not spec_files:
    errors.append(f"no spec files found in {specs_dir}")

for fname in spec_files:
    name = fname[:-3]
    spec_path = os.path.join(specs_dir, fname)
    spec_txt = open(spec_path, "r", encoding="utf-8").read()
    spec_hash = sha12(spec_path)

    m_tgt = re.search(r"^- \*\*Targets:\*\*\s*(.+)$", spec_txt, re.M)
    if not m_tgt:
        errors.append(f"{spec_path}: missing '- **Targets:**' line")
        continue
    tgts = [t.strip() for t in m_tgt.group(1).split(",") if t.strip()]
    bad_tgts = [t for t in tgts if t not in ("worker", "local")]
    if bad_tgts or not tgts:
        errors.append(f"{spec_path}: invalid targets {tgts}")
        continue

    assets = []
    m_assets = re.search(r"^- \*\*Assets:\*\*\s*(.+)$", spec_txt, re.M)
    if m_assets:
        raw_assets = m_assets.group(1).strip()
        if not raw_assets.lower().startswith("references "):
            assets = re.findall(r"`([^`]+)`", raw_assets)

    for target in tgts:
        expected[target].add(name)
        root = worker_root if target == "worker" else local_root
        skill_dir = os.path.join(root, name)
        skill_md = os.path.join(skill_dir, "SKILL.md")
        rel_md = os.path.relpath(skill_md, repo)

        if not os.path.isfile(skill_md):
            errors.append(f"missing compiled skill: {rel_md}")
            continue

        txt = open(skill_md, "r", encoding="utf-8").read()
        lines = txt.splitlines()

        m_fm = re.match(r"^---\n(.*?)\n---\n", txt, re.S)
        if not m_fm:
            errors.append(f"{rel_md}: missing YAML frontmatter")
        else:
            fm = m_fm.group(1)
            if len(fm) > 1024:
                errors.append(f"{rel_md}: frontmatter exceeds 1024 chars ({len(fm)})")
            keys = [
                line.split(":", 1)[0].strip()
                for line in fm.splitlines()
                if ":" in line and not line.startswith(" ")
            ]
            if keys != ["name", "description"]:
                errors.append(f"{rel_md}: frontmatter keys must be [name, description], got {keys}")
            m_name = re.search(r"^name:\s*(.+)$", fm, re.M)
            if not m_name or m_name.group(1).strip().strip("\"'") != name:
                errors.append(f"{rel_md}: frontmatter name does not match '{name}'")
            m_desc = re.search(r"^description:\s*(.+)$", fm, re.M)
            if not m_desc or not m_desc.group(1).strip().strip("\"'").startswith("Use when "):
                errors.append(f"{rel_md}: description must start with 'Use when '")

        if "<!-- REVIEW:" in txt:
            errors.append(f"{rel_md}: unstripped <!-- REVIEW: ... --> comment")

        exp_footer = (
            f"<!-- compiled-from: skills/specs/{name}.md@{spec_hash} "
            f"profile:{target}@{prof_sha.get(target, '')} -->"
        )
        if not lines or lines[-1] != exp_footer:
            got = lines[-1] if lines else "<empty>"
            errors.append(f"{rel_md}: stale or missing footer\n  got:      {got}\n  expected: {exp_footer}")

        if target == "local":
            for dirpath, _, filenames in os.walk(skill_dir):
                for fn in sorted(filenames):
                    fp = os.path.join(dirpath, fn)
                    try:
                        ftxt = open(fp, "r", encoding="utf-8").read()
                    except UnicodeDecodeError:
                        continue
                    leaks = sorted(set(m.lower() for m in local_leak.findall(ftxt)))
                    if leaks:
                        errors.append(
                            f"{os.path.relpath(fp, repo)}: forbidden worker tokens in local skill dir: {leaks}"
                        )
        else:
            if "@%UR_SKILL_HOOKS%" in txt and "Optional Hook Files" not in txt:
                errors.append(f"{rel_md}: references @%UR_SKILL_HOOKS% without Optional Hook Files rule")
            if name.startswith("brain") and "`/brain`" in txt:
                errors.append(
                    f"{rel_md}: bare `/brain` path (use `/brain/` so workerd does not rewrite to /ur:brain)"
                )

        for asset in assets:
            ap = os.path.join(skill_dir, asset)
            if not os.path.isfile(ap):
                errors.append(f"{rel_md}: missing declared asset {os.path.relpath(ap, repo)}")

# Check for orphan compiled directories
for target, root in (("worker", worker_root), ("local", local_root)):
    if not os.path.isdir(root):
        errors.append(f"missing output root: {os.path.relpath(root, repo)}")
        continue
    actual = {
        d for d in os.listdir(root) if not d.startswith(".") and os.path.isdir(os.path.join(root, d))
    }
    orphans = sorted(actual - expected[target])
    if orphans:
        errors.append(f"orphan {target} skill directories in {os.path.relpath(root, repo)}: {orphans}")

# Verify every default strategy skill exists in worker potential-skills or vendored superpowers
if not os.path.isfile(strategy_rs):
    errors.append("missing crates/server/src/strategy.rs")
else:
    src = open(strategy_rs, "r", encoding="utf-8").read()
    prod = src.split("#[cfg(test)]", 1)[0]
    m_skills = re.search(r"\bpub fn skills\b.*", prod, re.S)
    if not m_skills:
        errors.append("crates/server/src/strategy.rs: could not locate 'pub fn skills'")
    else:
        body = re.sub(r"//.*", "", m_skills.group(0))
        vendor_skills = set()
        if os.path.isdir(vendor_root):
            for d in os.listdir(vendor_root):
                if not d.startswith(".") and os.path.isfile(os.path.join(vendor_root, d, "SKILL.md")):
                    vendor_skills.add(d)
        available = expected["worker"] | vendor_skills
        referenced = set(re.findall(r'"([a-z0-9:_-]+)"\.(?:into|to_string)\(\)', body))
        referenced |= set(re.findall(r'String::from\("([a-z0-9:_-]+)"\)', body))
        if not referenced:
            errors.append("crates/server/src/strategy.rs: extracted 0 skill names from skills()/common_skills()")
        for skill_name in sorted(referenced):
            if skill_name not in available:
                errors.append(
                    f"crates/server/src/strategy.rs references '{skill_name}', "
                    "which is absent from potential-skills and vendor/superpowers"
                )

if errors:
    for err in errors:
        print(f"ERROR: {err}", file=sys.stderr)
    print(f"check-skills: FAILED ({len(errors)} issue(s))", file=sys.stderr)
    sys.exit(1)

print(
    f"check-skills: OK ({len(spec_files)} specs -> "
    f"{len(expected['worker'])} worker, {len(expected['local'])} local)"
)
PY
