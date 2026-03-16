use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::panic;
use std::path::PathBuf;

use container::{BuildOpts, ContainerId, ContainerRuntime, ExecOpts, RunOpts, runtime_from_env};

fn test_context() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .canonicalize()
        .expect("tests/fixtures directory must exist")
}

/// Generate a short random prefix to avoid container name collisions.
fn random_prefix() -> String {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(std::process::id() as u64);
    format!("ur-e2e-{:04x}", hasher.finish() & 0xFFFF)
}

/// Force-remove a container by name, ignoring errors (container may not exist).
fn force_remove_container(rt: &impl ContainerRuntime, name: &str) {
    let id = ContainerId(name.to_string());
    let _ = rt.stop(&id);
    let _ = rt.rm(&id);
}

#[test]
fn build_run_stop_rm() {
    let rt = runtime_from_env();
    let context = test_context();
    let container_name = format!("{}-lifecycle", random_prefix());

    // Clean up any stale container from a previous failed run
    force_remove_container(&rt, &container_name);

    // Build
    let image = rt
        .build(&BuildOpts {
            tag: "ur-lifecycle-test:latest".into(),
            dockerfile: context.join("Dockerfile"),
            context,
        })
        .expect("build should succeed");

    // Run — use catch_unwind so we always clean up the container on failure
    let id = rt
        .run(&RunOpts {
            image,
            name: container_name.clone(),
            cpus: 1,
            memory: "512M".into(),
            volumes: vec![],
            port_maps: vec![],
            env_vars: vec![],
            workdir: None,
            command: vec!["sleep".into(), "30".into()],
            network: None,
            add_hosts: vec![],
        })
        .expect("run should succeed");

    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
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
    }));

    // Always clean up
    rt.stop(&id).expect("stop should succeed");
    rt.rm(&id).expect("rm should succeed");

    // Re-raise if the test body panicked
    if let Err(e) = result {
        panic::resume_unwind(e);
    }
}
