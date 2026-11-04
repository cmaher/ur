//! Integration tests against a real tmux server on a throwaway socket, with a
//! stub agent in place of `claude`. Requires `tmux` on PATH (as the tmux crate
//! tests do). Never touches the `ur` socket, `~/.ur` or `~/.claude`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tempfile::TempDir;
use tonic::Request;
use ur_config::AgentType;
use ur_rpc::proto::workerd::worker_daemon_service_server::WorkerDaemonService;
use ur_rpc::proto::workerd::{NotifyIdleRequest, SendMessageRequest};

use super::*;
use crate::local_settings::LocalSettingsManager;
use crate::local_workspace::LocalMount;
use crate::workerd_router::WorkerdRouter;

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

struct Fixture {
    tmp: TempDir,
    socket: String,
    manager: LocalWorkerManager,
    registry: LocalWorkerRegistry,
    _guard: ServerGuard,
}

fn write_exec(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Fixture {
    fn new(tag: &str, exit_poll: Duration) -> Self {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let socket = format!("ur-test-lw-{tag}-{}-{nanos}", std::process::id());
        for dir in ["shared", "config", "workspace", "home", "out", "src/ctx"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        // Records cwd and env, then stays alive as an interactive shell.
        write_exec(
            &root.join("bin/stub-agent"),
            "#!/bin/sh\npwd > \"$STUB_OUT/$UR_WORKER_ID.pwd\"\n\
             env > \"$STUB_OUT/$UR_WORKER_ID.env\"\nexec sh\n",
        );
        let registry = LocalWorkerRegistry::default();
        let manager = LocalWorkerManager::new(
            LocalWorkspaceManager::new(
                root.join("config"),
                root.join("workspace"),
                root.join("home"),
                root.join("shared"),
                LocalSettingsManager,
            ),
            registry.clone(),
            socket.clone(),
            exit_poll,
            "127.0.0.1:2".into(),
            Some(root.join("bin/stub-agent")),
        );
        Fixture {
            tmp,
            socket: socket.clone(),
            manager,
            registry,
            _guard: ServerGuard { socket },
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().canonicalize().unwrap().join(rel)
    }

    /// A slot with a `.git/hooks` dir; returns its path.
    fn slot(&self, name: &str) -> PathBuf {
        let slot = self.path(name);
        fs::create_dir_all(slot.join(".git/hooks")).unwrap();
        slot
    }

    fn request(&self, id: &str, slot: &Path) -> LocalWorkspaceRequest {
        LocalWorkspaceRequest {
            worker_id: id.into(),
            slot_path: slot.to_path_buf(),
            agent: AgentType::Claude,
            model: None,
            effort: String::new(),
            mounts: vec![LocalMount {
                source: self.path("src/ctx"),
                dest: "/ctx".into(),
                read_only: false,
            }],
            memory_dir: None,
            brain_dir: None,
            skills: Vec::new(),
            host_skills: Vec::new(),
            instruction_strategy: None,
            project_key: "p".into(),
            project_instruction_path: None,
            allow_no_verify: false,
        }
    }

    fn env(&self) -> Vec<(String, String)> {
        vec![
            (
                "STUB_OUT".into(),
                self.path("out").to_string_lossy().into_owned(),
            ),
            (ur_config::UR_WORKER_ID_ENV.into(), "spoofed".into()),
            (ur_config::UR_SERVER_ADDR_ENV.into(), "127.0.0.1:1".into()),
            (ur_config::UR_WORKER_SECRET_ENV.into(), "secret".into()),
            (ur_config::UR_WORKERD_ADDR_ENV.into(), "spoofed:9".into()),
        ]
    }

    fn session(&self, id: &str) -> tmux::Session {
        tmux::Session::on_socket(&self.socket, &session_name(id))
    }

    fn drive(&self, id: &str) -> PathBuf {
        self.path("workspace/drive").join(id)
    }

    async fn assert_gone(&self, id: &str) {
        assert!(!self.session(id).exists().await.unwrap(), "session {id}");
        assert!(!self.registry.contains(id), "registry {id}");
        assert!(!self.drive(id).exists(), "drive {id}");
        assert!(self.path("src/ctx").is_dir(), "mount source survives");
    }
}

async fn wait_for(what: &str, mut check: impl AsyncFnMut() -> bool) {
    for _ in 0..200 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

fn routed<T>(id: &str, msg: T) -> Request<T> {
    let mut req = Request::new(msg);
    req.metadata_mut()
        .insert(ur_config::WORKER_ID_HEADER, id.parse().unwrap());
    req
}

fn send(text: &str) -> SendMessageRequest {
    SendMessageRequest {
        message: text.into(),
        submit: Some(true),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn launch_routes_two_workers_independently_and_stops() {
    let f = Fixture::new("route", Duration::from_secs(3600));
    let (slot1, slot2) = (f.slot("slot1"), f.slot("slot2"));
    write_exec(
        &slot1.join("ur-hooks/startup-bg/10-bg"),
        "#!/bin/sh\npwd > \"$STUB_OUT/bg-ran\"\n",
    );
    let (req1, req2) = (f.request("w1", &slot1), f.request("w2", &slot2));
    let (r1, r2) = tokio::join!(
        f.manager.launch(&req1, f.env()),
        f.manager.launch(&req2, f.env()),
    );
    assert_eq!(r1.unwrap(), f.drive("w1"));
    assert_eq!(r2.unwrap(), f.drive("w2"));

    for (id, slot) in [("w1", &slot1), ("w2", &slot2)] {
        let pwd = f.path(&format!("out/{id}.pwd"));
        wait_for("stub agent", async || pwd.exists()).await;
        let cwd = PathBuf::from(fs::read_to_string(&pwd).unwrap().trim());
        assert_eq!(cwd.canonicalize().unwrap(), *slot);
        let env = fs::read_to_string(f.path(&format!("out/{id}.env"))).unwrap();
        let drive = f.drive(id);
        for line in [
            format!("UR_WORKER_ID={id}"),
            "UR_AGENT_TYPE=claude".into(),
            format!("UR_DRIVE={}", drive.display()),
            "UR_RUNTIME=local".into(),
            "UR_SERVER_ADDR=127.0.0.1:1".into(),
            "UR_WORKERD_ADDR=127.0.0.1:2".into(),
        ] {
            assert!(env.lines().any(|l| l == line), "{id} env missing {line}");
        }
        assert!(drive.join("ctx").is_symlink());
    }
    let bg = f.path("out/bg-ran");
    wait_for("bg hook", async || {
        fs::read_to_string(&bg).is_ok_and(|s| s.ends_with('\n'))
    })
    .await;
    let bg_cwd = PathBuf::from(fs::read_to_string(&bg).unwrap().trim());
    assert_eq!(bg_cwd.canonicalize().unwrap(), slot1, "bg hook cwd");

    let router = WorkerdRouter {
        registry: f.registry.clone(),
    };
    router
        .notify_idle(routed("w1", NotifyIdleRequest {}))
        .await
        .unwrap();
    for id in ["w1", "w2"] {
        let resp = router
            .send_message(routed(id, send(&format!("echo routed-{id}"))))
            .await
            .unwrap()
            .into_inner();
        assert!(resp.success, "{}", resp.error);
    }
    for (id, other) in [("w1", "w2"), ("w2", "w1")] {
        let session = f.session(id);
        let marker = format!("routed-{id}");
        wait_for("routed echo", async || {
            session.capture_pane().await.unwrap().contains(&marker)
        })
        .await;
        let pane = session.capture_pane().await.unwrap();
        assert!(!pane.contains(&format!("routed-{other}")), "{id}: {pane}");
    }
    let err = router
        .notify_idle(routed("nope", NotifyIdleRequest {}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
    let err = router
        .notify_idle(Request::new(NotifyIdleRequest {}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    assert_eq!(
        f.manager.list().await.unwrap(),
        vec![("w1".to_owned(), true), ("w2".to_owned(), true)]
    );

    f.manager.stop("w1").await.unwrap();
    f.assert_gone("w1").await;
    let err = router
        .notify_idle(routed("w1", NotifyIdleRequest {}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
    assert!(f.session("w2").exists().await.unwrap());
    f.manager.stop("w1").await.unwrap();
    f.manager.stop("w2").await.unwrap();
    f.assert_gone("w2").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_launch_is_rejected_without_touching_live_worker() {
    let f = Fixture::new("dup", Duration::from_secs(3600));
    let slot = f.slot("slot");
    f.manager
        .launch(&f.request("w1", &slot), f.env())
        .await
        .unwrap();
    let err = f
        .manager
        .launch(&f.request("w1", &slot), f.env())
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::AlreadyExists);
    assert!(f.session("w1").exists().await.unwrap());
    assert!(f.registry.contains("w1"));
    assert!(f.drive("w1").join("ctx").is_symlink());
    f.manager.stop("w1").await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_launches_leave_nothing_behind() {
    let f = Fixture::new("fail", Duration::from_secs(3600));
    let slot = f.slot("slot");

    let req = LocalWorkspaceRequest {
        agent: AgentType::Codex,
        ..f.request("w1", &slot)
    };
    let err = f.manager.launch(&req, f.env()).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    f.assert_gone("w1").await;

    let req = LocalWorkspaceRequest {
        mounts: vec![LocalMount {
            source: f.path("src/missing"),
            dest: "/x".into(),
            read_only: false,
        }],
        ..f.request("w1", &slot)
    };
    let err = f.manager.launch(&req, f.env()).await.unwrap_err();
    assert!(err.message().contains("src/missing"), "{err}");
    f.assert_gone("w1").await;

    write_exec(
        &slot.join("ur-hooks/startup/10-fail"),
        "#!/bin/sh\necho hook-stderr-text >&2\nexit 3\n",
    );
    let err = f
        .manager
        .launch(&f.request("w1", &slot), f.env())
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert!(err.message().contains("sync startup hooks"), "{err}");
    assert!(err.message().contains("hook-stderr-text"), "{err}");
    f.assert_gone("w1").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_exit_tears_worker_down() {
    let f = Fixture::new("exit", Duration::from_millis(100));
    let slot = f.slot("slot");
    f.manager
        .launch(&f.request("w1", &slot), f.env())
        .await
        .unwrap();
    f.session("w1").kill().await.unwrap();
    wait_for("exit teardown", async || {
        !f.registry.contains("w1") && !f.drive("w1").exists()
    })
    .await;
    f.assert_gone("w1").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_tmux_renamed_ids_and_missing_server_env() {
    let f = Fixture::new("invalid", Duration::from_secs(3600));
    let slot = f.slot("slot");
    for id in ["w.1", "w:1"] {
        let err = f
            .manager
            .launch(&f.request(id, &slot), f.env())
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument, "{id}: {err}");
        f.assert_gone(id).await;
        assert!(f.manager.list().await.unwrap().is_empty());
        assert!(
            tmux::Session::list_sessions(Some(&f.socket))
                .await
                .unwrap()
                .is_empty(),
            "{id}: renamed session leaked"
        );
    }
    for key in [
        ur_config::UR_SERVER_ADDR_ENV,
        ur_config::UR_WORKER_SECRET_ENV,
    ] {
        for env in [
            f.env()
                .into_iter()
                .filter(|(k, _)| k != key)
                .collect::<Vec<_>>(),
            f.env()
                .into_iter()
                .map(|(k, v)| if k == key { (k, String::new()) } else { (k, v) })
                .collect(),
        ] {
            let err = f
                .manager
                .launch(&f.request("w1", &slot), env)
                .await
                .unwrap_err();
            assert_eq!(err.code(), tonic::Code::InvalidArgument, "{key}: {err}");
            assert!(err.message().contains(key), "{err}");
            f.assert_gone("w1").await;
        }
    }
}
