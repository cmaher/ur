use std::path::PathBuf;

use container::{BuildOpts, RunOpts, runtime_from_env};

fn claude_worker_context() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../containers/claude-worker")
        .canonicalize()
        .expect("containers/claude-worker directory must exist")
}

#[test]
fn build_run_stop_rm() {
    let rt = runtime_from_env();
    let context = claude_worker_context();

    // Build
    let image = rt
        .build(&BuildOpts {
            tag: "ur-worker-test:latest".into(),
            dockerfile: context.join("Dockerfile"),
            context: context.clone(),
        })
        .expect("build should succeed");

    // Run (use sleep instead of tmux for a simpler test container)
    let id = rt
        .run(&RunOpts {
            image,
            name: "ur-test-lifecycle".into(),
            cpus: 1,
            memory: "512M".into(),
            volumes: vec![],
            socket_mounts: vec![],
            workdir: None,
            command: vec!["sleep".into(), "30".into()],
        })
        .expect("run should succeed");

    // Stop
    rt.stop(&id).expect("stop should succeed");

    // Remove
    rt.rm(&id).expect("rm should succeed");
}
