# worker-agy (Container Image)

AGY-specific layer on `ur-worker-base:latest`, tagged `ur-worker-agy:latest`.

- Google's installer is vendored at `vendor/agy/install.sh` and must be invoked with bash:
  despite its `/bin/sh` declaration it uses `pipefail` and `local`, which fail under Debian's
  dash. The installer resolves the architecture-specific manifest, verifies SHA-512, and a
  failed refresh fails the image build. There is no published musl build.
- `CACHEBUST` refreshes AGY by rerunning the installer, never by calling `agy update`; use
  `cargo make install-update-agy` to force that layer without rebuilding the base image.
- `AGY_CLI_DISABLE_AUTO_UPDATE=1` prevents runtime update traffic.
- The image bakes `cache/onboarding.json` and runtime `settings.json` to suppress onboarding
  and trust prompts. Do not bake `jetski_state.pbtxt`; it contains per-installation state.
- `/home/worker/.gemini`, `.gemini/antigravity-cli/cache`, and `.gemini/config` are created
  and owned by `worker` before any credential mount is attached.
- The global hook lives at `~/.gemini/config/hooks.json`. Its format is named-hook keyed;
  `Stop` contains a flat handler list and runs `workertools notify-idle --json`. AGY runs the
  hook synchronously from the directory containing `hooks.json`.
- Repo-local `.agents/hooks.json` hooks can execute arbitrary commands when AGY loads them.
  A same-named repo hook does not replace the ur hook; configurations merge and both run.
  Loading under preconfigured workspace trust is inconsistent in AGY 1.2.0, so treat absent
  repo-hook execution as a bug, not a security boundary. A denial-of-service hook returning
  `{"decision":"continue"}` remains an unproven residual risk.
