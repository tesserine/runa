use libagent::ForgeConfig;
use runa_forge_compose::runtime_from_config;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;

#[test]
fn github_config_loads_a_forge_runtime_with_output_schemas() {
    let _env = EnvGuard::unset();
    let runtime = runtime_from_config(&ForgeConfig {
        forge_type: Some("github".to_string()),
        owner: Some("tesserine".to_string()),
        name: Some("runa".to_string()),
        api_base: Some("https://api.github.test".to_string()),
        ..ForgeConfig::default()
    })
    .unwrap()
    .expect("github config should load a connector");

    let read_ticket = runtime.tools.get("read-ticket").unwrap();
    assert_eq!(runtime.tools.len(), 8);
    assert_eq!(read_ticket.operation.canonical_name(), "read-ticket");
    assert!(read_ticket.output_schema.get("$defs").is_some());
}

#[test]
fn sourcehut_config_loads_a_forge_runtime() {
    let _env = EnvGuard::unset();
    let runtime = runtime_from_config(&ForgeConfig {
        forge_type: Some("sourcehut".to_string()),
        tracker_id: Some("4".to_string()),
        api_base: Some("https://todo.example/query".to_string()),
        git_remote: Some("ssh://git@git.example/runa".to_string()),
        ..ForgeConfig::default()
    })
    .unwrap()
    .expect("sourcehut config should load a connector");

    assert_eq!(runtime.tools.len(), 8);
    assert!(runtime.tools.contains_key("create-ticket"));
}

#[test]
fn explicit_aliases_are_applied_at_composition() {
    let _env = EnvGuard::unset();
    let mut aliases = BTreeMap::new();
    aliases.insert(
        "github:read-ticket".to_string(),
        "work-unit-read-ticket".to_string(),
    );
    let runtime = runtime_from_config(&ForgeConfig {
        forge_type: Some("github".to_string()),
        owner: Some("tesserine".to_string()),
        name: Some("runa".to_string()),
        tool_aliases: aliases,
        ..ForgeConfig::default()
    })
    .unwrap()
    .expect("github config should load a connector");

    assert!(!runtime.tools.contains_key("read-ticket"));
    assert!(runtime.tools.contains_key("work-unit-read-ticket"));
}

#[test]
fn absent_connector_config_leaves_the_mcp_surface_unchanged() {
    let _env = EnvGuard::unset();
    assert!(
        runtime_from_config(&ForgeConfig::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn environment_identity_without_file_forge_config_leaves_the_mcp_surface_unchanged() {
    let _env = EnvGuard::set(&[
        ("RUNA_FORGE_TYPE", "github"),
        ("RUNA_FORGE_OWNER", "override-owner"),
        ("RUNA_FORGE_NAME", "override-repo"),
    ]);

    assert!(
        runtime_from_config(&ForgeConfig::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn github_connector_coordinates_follow_resolved_env_identity() {
    let _env = EnvGuard::set(&[
        ("RUNA_FORGE_TYPE", "github"),
        ("RUNA_FORGE_OWNER", "override-owner"),
        ("RUNA_FORGE_NAME", "override-repo"),
    ]);
    let (api_base, request_capture, server) =
        http_json_server(r#"{"number":203,"title":"Override","body":"body","state":"open"}"#);

    let runtime = runtime_from_config(&ForgeConfig {
        forge_type: Some("github".to_string()),
        owner: Some("stale-owner".to_string()),
        name: Some("stale-repo".to_string()),
        api_base: Some(api_base),
        ..ForgeConfig::default()
    })
    .unwrap()
    .expect("github connector should compose from resolved identity");

    let output = runtime
        .call_tool("read-ticket", json!({ "reference": "203" }))
        .unwrap();

    assert!(
        request_capture
            .lock()
            .unwrap()
            .starts_with("GET /repos/override-owner/override-repo/issues/203 ")
    );
    assert_eq!(
        output["handle"]["id"],
        "github:override-owner/override-repo:issue:203"
    );
    server.join().unwrap();
}

#[test]
fn sourcehut_graphql_coordinates_follow_resolved_env_identity() {
    let _env = EnvGuard::set(&[
        ("RUNA_FORGE_TYPE", "sourcehut"),
        ("RUNA_FORGE_OWNER", "override-owner"),
        ("RUNA_FORGE_NAME", "override-repo"),
        ("RUNA_FORGE_TRACKER_ID", "9"),
    ]);
    let (api_base, request_capture, server) = http_json_server(
        r#"{"data":{"tracker":{"ticket":{"id":203,"subject":"Override","description":"body","status":"open"}}}}"#,
    );

    let runtime = runtime_from_config(&ForgeConfig {
        forge_type: Some("sourcehut".to_string()),
        owner: Some("stale-owner".to_string()),
        name: Some("stale-repo".to_string()),
        tracker_id: Some("4".to_string()),
        api_base: Some(format!("{api_base}/query")),
        ..ForgeConfig::default()
    })
    .unwrap()
    .expect("sourcehut connector should compose from resolved identity");

    let output = runtime
        .call_tool("read-ticket", json!({ "reference": "203" }))
        .unwrap();

    let request = request_capture.lock().unwrap().clone();
    assert!(request.starts_with("POST /query "));
    assert!(request.contains(r#""tracker":"9""#), "request: {request}");
    assert_eq!(output["handle"]["id"], "sourcehut:tracker:9:ticket:203");
    server.join().unwrap();
}

#[test]
fn sourcehut_git_remote_repo_follows_resolved_env_identity() {
    let _env = EnvGuard::set(&[
        ("RUNA_FORGE_TYPE", "sourcehut"),
        ("RUNA_FORGE_OWNER", "override-owner"),
        ("RUNA_FORGE_NAME", "override-repo"),
        ("RUNA_FORGE_TRACKER_ID", "9"),
    ]);
    let temp = tempfile::tempdir().unwrap();
    let stale_remote = temp.path().join("stale-owner").join("stale-repo");
    let override_remote = temp.path().join("override-owner").join("override-repo");
    init_bare_remote(&stale_remote);
    init_bare_remote(&override_remote);
    let commit = git(
        &["rev-parse", "HEAD"],
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );

    let runtime = runtime_from_config(&ForgeConfig {
        forge_type: Some("sourcehut".to_string()),
        owner: Some("stale-owner".to_string()),
        name: Some("stale-repo".to_string()),
        tracker_id: Some("4".to_string()),
        api_base: Some("http://127.0.0.1:1/query".to_string()),
        git_remote: Some(stale_remote.to_string_lossy().into_owned()),
        ..ForgeConfig::default()
    })
    .unwrap()
    .expect("sourcehut connector should compose from resolved identity");

    let output = runtime
        .call_tool(
            "deliver-change-proposal",
            json!({
                "work_unit": {
                    "id": "sourcehut:tracker:9:ticket:203",
                    "display": "sourcehut:9#203"
                },
                "branch": "issue-203",
                "commit": commit,
                "base": "main",
                "summary": "summary",
                "body": "body",
                "version": 1
            }),
        )
        .unwrap();

    assert_eq!(output["commit"], commit);
    assert!(remote_ref_exists(&override_remote, "refs/heads/issue-203"));
    assert!(!remote_ref_exists(&stale_remote, "refs/heads/issue-203"));
}

fn http_json_server(body: &'static str) -> (String, Arc<Mutex<String>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let request_capture = Arc::new(Mutex::new(String::new()));
    let thread_capture = Arc::clone(&request_capture);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let size = stream.read(&mut request).unwrap();
        *thread_capture.lock().unwrap() = String::from_utf8_lossy(&request[..size]).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    (format!("http://{address}"), request_capture, server)
}

fn init_bare_remote(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    git(
        &["init", "--bare", path.to_str().unwrap()],
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );
}

fn remote_ref_exists(remote: &Path, reference: &str) -> bool {
    Command::new("git")
        .args(["ls-remote", remote.to_str().unwrap(), reference])
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

fn git(args: &[&str], cwd: &Path) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn unset() -> Self {
        Self::apply(&[])
    }

    fn set(values: &[(&'static str, &str)]) -> Self {
        Self::apply(values)
    }

    fn apply(values: &[(&'static str, &str)]) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let names = [
            "RUNA_FORGE_TYPE",
            "RUNA_FORGE_OWNER",
            "RUNA_FORGE_NAME",
            "RUNA_FORGE_TRACKER_ID",
        ];
        let previous = names
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect::<Vec<_>>();
        for name in names {
            unsafe { std::env::remove_var(name) };
        }
        for (name, value) in values {
            unsafe { std::env::set_var(name, value) };
        }
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in &self.previous {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
