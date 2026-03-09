use std::path::PathBuf;

use container::{BuildOpts, ContainerRuntime, ExecOpts, RunOpts, runtime_from_env};

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
            port_maps: vec![],
            env_vars: vec![],
            workdir: None,
            command: vec!["sleep".into(), "30".into()],
            network: None,
        })
        .expect("run should succeed");

    // Exec
    let output = rt
        .exec(
            &id,
            &ExecOpts {
                command: vec!["echo".into(), "hello".into()],
                workdir: None,
            },
        )
        .expect("exec should succeed");
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout.trim(), "hello");
    assert!(output.stderr.is_empty());

    // Exec with non-zero exit
    let output = rt
        .exec(
            &id,
            &ExecOpts {
                command: vec!["sh".into(), "-c".into(), "exit 42".into()],
                workdir: None,
            },
        )
        .expect("exec should succeed even with non-zero exit");
    assert_eq!(output.exit_code, 42);

    // Stop
    rt.stop(&id).expect("stop should succeed");

    // Remove
    rt.rm(&id).expect("rm should succeed");
}
