//! Integration tests against a real tmux server on a throwaway socket.
//!
//! Requires `tmux` on PATH. Each test uses a unique socket name and kills its
//! server on exit, so it never touches the user's default tmux server.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tmux::{CreateOptions, Session};

/// Kills the test's tmux server when dropped, even if an assertion panics.
struct ServerGuard {
    socket: String,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .output();
    }
}

fn unique_socket(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    format!("ur-test-{tag}-{}-{nanos}", std::process::id())
}

#[tokio::test]
async fn create_list_window_kill_on_dedicated_socket() {
    let socket = unique_socket("lifecycle");
    let _guard = ServerGuard {
        socket: socket.clone(),
    };
    let start_dir = std::env::temp_dir().canonicalize().expect("temp dir");

    assert!(
        Session::list_sessions(Some(&socket))
            .await
            .expect("list on absent server")
            .is_empty(),
        "a server that is not running has no sessions"
    );

    let session = Session::create(CreateOptions {
        name: "ur-w1".into(),
        socket: Some(socket.clone()),
        start_dir: Some(start_dir.clone()),
        env: vec![("UR_TMUX_TEST".into(), "hello".into())],
        width: Some(120),
        height: Some(40),
        detached: true,
    })
    .await
    .expect("create session");

    assert_eq!(session.socket(), Some(socket.as_str()));
    assert!(session.exists().await.expect("has-session"));
    assert_eq!(
        Session::list_sessions(Some(&socket))
            .await
            .expect("list sessions"),
        vec!["ur-w1".to_string()]
    );

    let show = |args: &[&str]| {
        let out = std::process::Command::new("tmux")
            .args(["-L", &socket])
            .args(args)
            .output()
            .expect("run tmux");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert_eq!(
        show(&[
            "display-message",
            "-p",
            "-t",
            "ur-w1:0.0",
            "#{pane_current_path}"
        ]),
        start_dir.to_string_lossy()
    );
    assert_eq!(
        show(&["show-environment", "-t", "ur-w1", "UR_TMUX_TEST"]),
        "UR_TMUX_TEST=hello"
    );

    session.new_window("bg").await.expect("new window");
    assert_eq!(
        show(&["list-windows", "-t", "ur-w1", "-F", "#{window_name}"])
            .lines()
            .last(),
        Some("bg")
    );
    // Window 0 stays current, so lifecycle checks still target the agent pane.
    assert_eq!(
        show(&["display-message", "-p", "-t", "ur-w1", "#{window_index}"]),
        "0"
    );
    assert!(session.is_pane_alive().await.expect("pane alive"));

    session.kill().await.expect("kill session");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!session.exists().await.expect("has-session after kill"));
    assert!(
        Session::list_sessions(Some(&socket))
            .await
            .expect("list after kill")
            .is_empty()
    );

    // Killing an already-gone session is not an error.
    session.kill().await.expect("idempotent kill");
}

#[tokio::test]
async fn kill_prefix_does_not_match_other_session() {
    let socket = unique_socket("exact");
    let _guard = ServerGuard {
        socket: socket.clone(),
    };

    let keep = Session::create(CreateOptions {
        name: "ur-w10".into(),
        socket: Some(socket.clone()),
        detached: true,
        ..Default::default()
    })
    .await
    .expect("create ur-w10");

    // `ur-w1` does not exist; an inexact target would prefix-match `ur-w10`.
    let missing = Session::on_socket(&socket, "ur-w1");
    assert!(!missing.exists().await.expect("has-session"));
    missing.kill().await.expect("kill missing session");

    assert!(keep.exists().await.expect("ur-w10 survives"));
}
