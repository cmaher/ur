use std::path::PathBuf;

use container::{BuildOpts, RunOpts, runtime_from_env};

fn test_context() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .canonicalize()
        .expect("tests/fixtures directory must exist")
}

#[test]
fn build_run_stop_rm() {
    let rt = runtime_from_env();
    let context = test_context();

    // Build
    let image = rt
        .build(&BuildOpts {
            tag: "ur-lifecycle-test:latest".into(),
            dockerfile: context.join("Dockerfile"),
            context,
        })
        .expect("build should succeed");

    // Run
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
