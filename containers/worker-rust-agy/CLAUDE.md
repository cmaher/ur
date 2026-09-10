# worker-rust-agy (Container Image)

Extends `ur-worker-agy:latest` with the Rust-project entrypoint and is tagged
`ur-worker-rust-agy:latest`.

- It inherits AGY, its owned `.gemini` layout, baked onboarding/settings files, and trusted
  global `Stop` hook from `worker-agy`.
- Rust tooling runs on the host through workerd-generated hostexec shims; no Rust toolchain is
  installed in this image.
- The entrypoint runs `workerd init`, starts cargo-sweep and bacon in the background, then
  execs `workerd daemon` so workerd remains PID 1.
