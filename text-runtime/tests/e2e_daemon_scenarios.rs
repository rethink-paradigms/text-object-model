use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;

mod common;

use text_runtime::daemon::config::DaemonConfig;
use text_runtime::daemon::protocol::{Request, Response};

/// Locate the `text-runtime-daemon` binary for process-isolated tests.
///
/// `cargo test` sets `CARGO_BIN_EXE_text_runtime_daemon`, but plain
/// `cargo build --all-targets` / `cargo clippy --all-targets` do not, so we
/// fall back to deriving the path from the test executable's own location
/// (target/<profile>/deps/<test> → target/<profile>/text-runtime-daemon).
fn daemon_bin_path() -> PathBuf {
    if let Some(p) = option_env!("CARGO_BIN_EXE_text_runtime_daemon") {
        return PathBuf::from(p);
    }
    std::env::current_exe()
        .expect("failed to read test executable path")
        .parent()
        .expect("test executable has no parent dir")
        .parent()
        .map(|dir| dir.join("text-runtime-daemon"))
        .expect("cannot locate text-runtime-daemon binary")
}

struct TestDaemon {
    _temp_dir: TempDir,
    canonical_path: PathBuf,
    socket_path: PathBuf,
    pandoc_port: u16,
    daemon_task: JoinHandle<Result<(), text_runtime::error::TextRuntimeError>>,
}

impl TestDaemon {
    async fn new(port: u16) -> Self {
        let temp = TempDir::new().expect("failed to create temp dir");
        let canonical_path = std::fs::canonicalize(temp.path()).unwrap();
        let socket_path = canonical_path.join("daemon.sock");
        let pid_path = canonical_path.join("text-runtime.pid");

        let config = DaemonConfig {
            socket_path: socket_path.clone(),
            pid_path: pid_path.clone(),
            shutdown_grace_seconds: 1,
            workspaces: vec![],
        };

        let daemon_task = tokio::spawn(text_runtime::daemon::run(config));

        // Wait for the socket to exist and bind
        let mut attempts = 0;
        while !socket_path.exists() && attempts < 20 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            attempts += 1;
        }

        if !socket_path.exists() {
            panic!("Daemon failed to bind socket within timeout");
        }

        Self {
            _temp_dir: temp,
            canonical_path,
            socket_path,
            pandoc_port: port,
            daemon_task,
        }
    }

    async fn connect(
        &self,
    ) -> (
        tokio::io::ReadHalf<UnixStream>,
        tokio::io::WriteHalf<UnixStream>,
    ) {
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .expect("Failed to connect to daemon socket");
        tokio::io::split(stream)
    }

    fn setup_workspace_dirs(&self, name: &str) -> (PathBuf, PathBuf) {
        let ws_root = self.canonical_path.join(format!("{}_root", name));
        let ws_data = self.canonical_path.join(format!("{}_data", name));
        fs::create_dir_all(&ws_root).unwrap();
        fs::create_dir_all(&ws_data).unwrap();

        // Write config.json for the workspace's Runtime to use the dedicated pandoc port
        let runtime_cfg_dir = ws_data.join(".textruntime");
        fs::create_dir_all(&runtime_cfg_dir).unwrap();
        fs::write(
            runtime_cfg_dir.join("config.json"),
            format!("{{\"pandoc_port\": {}}}\n", self.pandoc_port),
        )
        .unwrap();

        (ws_root, ws_data)
    }
}

async fn send_req(
    writer: &mut tokio::io::WriteHalf<UnixStream>,
    id: &str,
    cmd: &str,
    params: serde_json::Value,
) {
    let req = Request {
        id: id.to_string(),
        cmd: cmd.to_string(),
        params,
    };
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .await
        .expect("Failed to write request");
}

async fn recv_res(reader: &mut BufReader<tokio::io::ReadHalf<UnixStream>>) -> Response {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .expect("Failed to read response line");
    assert!(!line.is_empty(), "Socket EOF when waiting for response");
    serde_json::from_str(&line).expect("Failed to parse response JSON")
}

#[tokio::test]
async fn test_scenario_1_rapid_research_workflow() {
    let _ = tracing_subscriber::fmt::try_init();
    let daemon = TestDaemon::new(common::free_port()).await;
    let (ws_root, ws_data) = daemon.setup_workspace_dirs("research");

    let (read_half, mut write_half) = daemon.connect().await;
    let mut reader = BufReader::new(read_half);

    // 1. Add workspace
    send_req(
        &mut write_half,
        "req-1",
        "workspace_add",
        json!({
            "name": "research",
            "root": ws_root.to_str().unwrap(),
            "data_dir": ws_data.to_str().unwrap(),
            "watch_dirs": [ws_root.to_str().unwrap()],
        }),
    )
    .await;
    let res = recv_res(&mut reader).await;
    assert!(res.ok, "WorkspaceAdd failed: {:?}", res.error);

    // 2. Rapidly write multiple markdown files to trigger filesystem watcher
    let doc_contents = [
        ("file1.md", "# Document One\nThis is the first document about rust concurrency.\n"),
        ("file2.md", "# Document Two\nThis is the second document about databases and SQLite.\n"),
        ("file3.md", "# Document Three\nThis is the third document about full-text search indexers.\n"),
        ("file4.md", "# Document Four\nThis is the fourth document about tokio actors and channels.\n"),
        ("file5.md", "# Document Five\nThis is the fifth document containing deep architectural workflows.\n"),
    ];

    for (name, content) in &doc_contents {
        fs::write(ws_root.join(name), content).unwrap();
    }

    // 3. Immediately send concurrent searches while ingestion is likely happening.
    // We try to search multiple times with a small delay.
    let mut found_docs = false;
    let mut last_hits = vec![];
    for _ in 0..15 {
        send_req(
            &mut write_half,
            "search-req",
            "search",
            json!({
                "workspace": "research",
                "query": "document",
            }),
        )
        .await;
        let res = recv_res(&mut reader).await;
        if res.ok {
            if let Some(data) = res.data {
                if let Some(hits) = data.get("results").and_then(|v| v.as_array()) {
                    last_hits = hits.clone();
                    let doc_ids: std::collections::HashSet<String> = hits
                        .iter()
                        .map(|h| h["doc_id"].as_str().unwrap().to_string())
                        .collect();
                    if doc_ids.len() >= doc_contents.len() {
                        found_docs = true;
                        break;
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(
        found_docs,
        "Watcher failed to auto-ingest all documents within timeout. Last hits: {:?}",
        last_hits
    );

    // 4. Read one of the documents to get its sentence UUID
    send_req(
        &mut write_half,
        "search-file1",
        "search",
        json!({
            "workspace": "research",
            "query": "concurrency",
        }),
    )
    .await;
    let search_res = recv_res(&mut reader).await;
    println!("concurrency search result: {:?}", search_res);
    let results = search_res.data.as_ref().unwrap()["results"]
        .as_array()
        .unwrap();
    assert!(!results.is_empty(), "No search results for 'concurrency'");

    // Find the sentence node specifically
    let sentence_hit = results
        .iter()
        .find(|h| h["node_type"].as_str().unwrap() == "sentence")
        .expect("Failed to find sentence node in FTS hits");

    let doc_id = sentence_hit["doc_id"].as_str().unwrap().to_string();
    let sentence_uuid = sentence_hit["sentence_uuid"].as_str().unwrap().to_string();

    // Create an annotation on that sentence
    send_req(
        &mut write_half,
        "annotate-req",
        "annotate",
        json!({
            "workspace": "research",
            "doc_id": doc_id,
            "sentence_uuid": sentence_uuid,
            "body": "This sentence is very crucial to study concurrency.",
            "quote": "This is the first document about rust concurrency.",
        }),
    )
    .await;
    let anno_res = recv_res(&mut reader).await;
    assert!(anno_res.ok);

    // 5. Prepend paragraphs in file1.md to trigger a background re-ingest and verify anchoring shifts
    let modified_content = "# Document One\nPre-paragraph A\n\nPre-paragraph B\n\nThis is the first document about rust concurrency.\n";
    fs::write(ws_root.join("file1.md"), modified_content).unwrap();

    // Wait for watcher to trigger re-ingest (~500ms debounce + processing)
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Verify the annotation anchoring shifted correctly (sentence UUID remained stable)
    send_req(
        &mut write_half,
        "search-again-file1",
        "search",
        json!({
            "workspace": "research",
            "query": "concurrency",
        }),
    )
    .await;
    let search_again_res = recv_res(&mut reader).await;
    assert!(search_again_res.ok);
    let results_again = search_again_res.data.as_ref().unwrap()["results"]
        .as_array()
        .unwrap();

    let sentence_hit_again = results_again
        .iter()
        .find(|h| h["node_type"].as_str().unwrap() == "sentence")
        .expect("Failed to find sentence node after re-ingest");

    let new_sentence_uuid = sentence_hit_again["sentence_uuid"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        sentence_uuid, new_sentence_uuid,
        "Sentence UUID must remain stable across shifts for annotations to stay anchored"
    );

    // 6. Graceful shutdown
    send_req(&mut write_half, "shutdown-req", "shutdown", json!({})).await;
    let shutdown_res = recv_res(&mut reader).await;
    assert!(shutdown_res.ok);

    daemon.daemon_task.await.unwrap().expect("daemon crash");
}

#[tokio::test]
async fn test_scenario_2_chaos_and_concurrency() {
    let daemon = TestDaemon::new(common::free_port()).await;
    let (ws_root, ws_data) = daemon.setup_workspace_dirs("stress");

    // First setup the workspace using one connection
    {
        let (read_half, mut write_half) = daemon.connect().await;
        let mut reader = BufReader::new(read_half);
        send_req(
            &mut write_half,
            "setup",
            "workspace_add",
            json!({
                "name": "stress",
                "root": ws_root.to_str().unwrap(),
                "data_dir": ws_data.to_str().unwrap(),
            }),
        )
        .await;
        let res = recv_res(&mut reader).await;
        assert!(res.ok, "res not ok: {:?}", res.error);
    }

    // Now spawn 5 concurrent clients interacting with the daemon in different ways
    let socket_path = daemon.socket_path.clone();

    // Client A: Batch Ingest
    let client_a = tokio::spawn({
        let socket_path = socket_path.clone();
        async move {
            let stream = UnixStream::connect(socket_path).await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut rdr = BufReader::new(read);

            for i in 0..10 {
                send_req(
                    &mut write,
                    &format!("ingest-{}", i),
                    "ingest_text",
                    json!({
                        "workspace": "stress",
                        "text": format!("Stress test paragraph containing index token {}.", i),
                        "format": "markdown",
                        "title": format!("Doc {}", i),
                    }),
                )
                .await;
                let res = recv_res(&mut rdr).await;
                assert!(res.ok, "Client A failed at {}: {:?}", i, res.error);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    });

    // Client B: Continuous Search
    let client_b = tokio::spawn({
        let socket_path = socket_path.clone();
        async move {
            let stream = UnixStream::connect(socket_path).await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut rdr = BufReader::new(read);

            for i in 0..15 {
                send_req(
                    &mut write,
                    &format!("search-{}", i),
                    "search",
                    json!({
                        "workspace": "stress",
                        "query": "index",
                    }),
                )
                .await;
                let res = recv_res(&mut rdr).await;
                assert!(res.ok, "res not ok: {:?}", res.error);
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
        }
    });

    // Client C: Status Queries
    let client_c = tokio::spawn({
        let socket_path = socket_path.clone();
        async move {
            let stream = UnixStream::connect(socket_path).await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut rdr = BufReader::new(read);

            for i in 0..10 {
                send_req(&mut write, &format!("status-{}", i), "status", json!({})).await;
                let res = recv_res(&mut rdr).await;
                assert!(res.ok, "res not ok: {:?}", res.error);
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
        }
    });

    // Client D: Abrupt termination mid-request
    let client_d = tokio::spawn({
        let socket_path = socket_path.clone();
        async move {
            let stream = UnixStream::connect(socket_path).await.unwrap();
            let (_, mut write) = tokio::io::split(stream);

            // Write a partial, invalid payload and immediately drop/close the socket
            write
                .write_all(b"{\"id\": \"abrupt-1\", \"cmd\": \"ing")
                .await
                .unwrap();
            drop(write);
        }
    });

    // Client E: Mixed workspace status queries
    let client_e = tokio::spawn({
        let socket_path = socket_path.clone();
        async move {
            let stream = UnixStream::connect(socket_path).await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut rdr = BufReader::new(read);

            for i in 0..5 {
                send_req(
                    &mut write,
                    &format!("ws-list-{}", i),
                    "workspace_list",
                    json!({}),
                )
                .await;
                let res = recv_res(&mut rdr).await;
                assert!(res.ok, "res not ok: {:?}", res.error);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    });

    // Await all client tasks
    let (res_a, res_b, res_c, res_d, res_e) =
        tokio::join!(client_a, client_b, client_c, client_d, client_e);
    res_a.unwrap();
    res_b.unwrap();
    res_c.unwrap();
    res_d.unwrap();
    res_e.unwrap();

    // Shutdown daemon cleanly
    let (read, mut write) = daemon.connect().await;
    let mut rdr = BufReader::new(read);
    send_req(&mut write, "shutdown", "shutdown", json!({})).await;
    let res = recv_res(&mut rdr).await;
    assert!(res.ok, "res not ok: {:?}", res.error);

    daemon.daemon_task.await.unwrap().expect("daemon crashed");
}

#[tokio::test]
async fn test_scenario_3_hot_reload_evolution() {
    let _ = tracing_subscriber::fmt::try_init();

    let temp = TempDir::new().unwrap();
    let canonical_temp = std::fs::canonicalize(temp.path()).unwrap();

    // The daemon's SIGHUP reload handler re-reads the config from its default
    // XDG path via load_config(None) — that is $XDG_CONFIG_HOME/text-runtime/
    // config.toml. We point XDG_CONFIG_HOME at our temp dir, but only on the
    // CHILD process env (see below), never on this test process.
    let config_dir = canonical_temp.join("text-runtime");
    fs::create_dir_all(&config_dir).unwrap();
    let config_file = config_dir.join("config.toml");

    let socket_path = canonical_temp.join("daemon.sock");
    let pid_path = canonical_temp.join("text-runtime.pid");

    // Setup workspace directories under the temp path
    let ws_root1 = canonical_temp.join("ws1_root");
    let ws_data1 = canonical_temp.join("ws1_data");
    fs::create_dir_all(&ws_root1).unwrap();
    fs::create_dir_all(&ws_data1).unwrap();

    let ws_root2 = canonical_temp.join("ws2_root");
    let ws_data2 = canonical_temp.join("ws2_data");
    fs::create_dir_all(&ws_root2).unwrap();
    fs::create_dir_all(&ws_data2).unwrap();

    // Write config.json for the workspace Runtimes to use dedicated pandoc ports
    let rcfg1 = ws_data1.join(".textruntime");
    fs::create_dir_all(&rcfg1).unwrap();
    fs::write(
        rcfg1.join("config.json"),
        format!("{{\"pandoc_port\": {}}}\n", common::free_port()),
    )
    .unwrap();

    let rcfg2 = ws_data2.join(".textruntime");
    fs::create_dir_all(&rcfg2).unwrap();
    fs::write(
        rcfg2.join("config.json"),
        format!("{{\"pandoc_port\": {}}}\n", common::free_port()),
    )
    .unwrap();

    let initial_toml = format!(
        r#"
socket_path = "{}"
pid_path = "{}"
shutdown_grace_seconds = 2

[[workspaces]]
name = "ws1"
root = "{}"
data_dir = "{}"
watch_dirs = ["{}"]
"#,
        socket_path.display(),
        pid_path.display(),
        ws_root1.display(),
        ws_data1.display(),
        ws_root1.display(),
    );
    fs::write(&config_file, initial_toml).unwrap();

    // ── Spawn the daemon as a SEPARATE OS PROCESS ────────────────────────
    // SIGHUP is a process-global signal. The other three scenarios run their
    // daemons in-process (as tokio tasks in THIS test binary), so sending
    // SIGHUP to Pid::this() would trigger the reload handler of every daemon,
    // and each would drop the workspaces it does not manage — corrupting
    // scenarios 1 and 2 mid-test. Running this scenario's daemon in its own
    // process confines the signal to that one daemon.
    //
    // XDG_CONFIG_HOME is set on the child only, so no other code in this
    // test process can observe the temp config.
    let stderr_log = canonical_temp.join("daemon.stderr.log");
    let stderr_file = std::fs::File::create(&stderr_log).unwrap();
    let mut child = tokio::process::Command::new(daemon_bin_path())
        .arg("--config")
        .arg(&config_file)
        .env("XDG_CONFIG_HOME", &canonical_temp)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(stderr_file))
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn daemon process");

    // Wait for the child daemon to bind its socket. Give it a generous window:
    // the child opens two workspaces, each of which spawns a pandoc-server, so
    // startup can exceed 2-3s when the full test suite runs in parallel.
    let mut attempts = 0;
    while !socket_path.exists() && attempts < 100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        attempts += 1;
    }
    if !socket_path.exists() {
        let stderr = fs::read_to_string(&stderr_log).unwrap_or_default();
        panic!(
            "daemon process failed to bind socket within timeout.\nstderr:\n{}",
            stderr
        );
    }

    // Connect to check initial state
    {
        let (read_half, mut write_half) = UnixStream::connect(&socket_path)
            .await
            .map(tokio::io::split)
            .unwrap();
        let mut reader = BufReader::new(read_half);

        send_req(&mut write_half, "list-1", "workspace_list", json!({})).await;
        let res = recv_res(&mut reader).await;
        assert!(res.ok, "res not ok: {:?}", res.error);
        let list = res.data.unwrap()["workspaces"].as_array().unwrap().clone();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["name"].as_str().unwrap(), "ws1");
    }

    // Rewrite configuration file dynamically to add workspace 'ws2'
    let updated_toml = format!(
        r#"
socket_path = "{}"
pid_path = "{}"
shutdown_grace_seconds = 2

[[workspaces]]
name = "ws1"
root = "{}"
data_dir = "{}"
watch_dirs = ["{}"]

[[workspaces]]
name = "ws2"
root = "{}"
data_dir = "{}"
watch_dirs = ["{}"]
"#,
        socket_path.display(),
        pid_path.display(),
        ws_root1.display(),
        ws_data1.display(),
        ws_root1.display(),
        ws_root2.display(),
        ws_data2.display(),
        ws_root2.display(),
    );
    fs::write(&config_file, updated_toml).unwrap();

    // ── Send SIGHUP to the CHILD PROCESS ONLY ────────────────────────────
    // Previously this was kill(Pid::this(), SIGHUP), which delivered the
    // signal to every daemon in the test process. Targeting the child pid
    // confines the hot-reload to the daemon that owns this scenario.
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        let child_pid = child.id().expect("spawned daemon child has no pid") as i32;
        kill(Pid::from_raw(child_pid), Signal::SIGHUP).unwrap();
    }
    #[cfg(not(unix))]
    {
        panic!("SIGHUP hot-reload scenario requires a Unix platform");
    }

    // Poll until the reload handler has hot-loaded 'ws2' (it is async and
    // spawns a new pandoc-server, so a fixed sleep is too brittle under load).
    {
        let (read_half, mut write_half) = UnixStream::connect(&socket_path)
            .await
            .map(tokio::io::split)
            .unwrap();
        let mut reader = BufReader::new(read_half);
        let mut ws2_seen = false;
        for _ in 0..50 {
            send_req(&mut write_half, "poll-reload", "workspace_list", json!({})).await;
            let res = recv_res(&mut reader).await;
            assert!(res.ok, "res not ok: {:?}", res.error);
            if let Some(list) = res.data.unwrap()["workspaces"].as_array() {
                if list.iter().any(|w| w["name"].as_str() == Some("ws2")) {
                    ws2_seen = true;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            ws2_seen,
            "workspace 'ws2' was not hot-loaded after SIGHUP reload"
        );
    }

    // Connect and verify 'ws2' has been hot-loaded and is ready
    {
        let (read_half, mut write_half) = UnixStream::connect(&socket_path)
            .await
            .map(tokio::io::split)
            .unwrap();
        let mut reader = BufReader::new(read_half);

        send_req(&mut write_half, "list-2", "workspace_list", json!({})).await;
        let res = recv_res(&mut reader).await;
        assert!(res.ok, "res not ok: {:?}", res.error);
        let list = res.data.unwrap()["workspaces"].as_array().unwrap().clone();
        assert_eq!(list.len(), 2);

        let mut ws_names: Vec<String> = list
            .iter()
            .map(|v| v["name"].as_str().unwrap().to_string())
            .collect();
        ws_names.sort();
        assert_eq!(ws_names, vec!["ws1".to_string(), "ws2".to_string()]);

        // Write a file to ws2 root and see if it gets auto-ingested
        fs::write(
            ws_root2.join("hot.md"),
            "# Hot Reloaded File\nIt works fine.\n",
        )
        .unwrap();

        // Wait for watcher debounce (500ms) + process
        tokio::time::sleep(Duration::from_millis(800)).await;

        send_req(
            &mut write_half,
            "search-hot",
            "search",
            json!({
                "workspace": "ws2",
                "query": "Reloaded",
            }),
        )
        .await;
        let search_res = recv_res(&mut reader).await;
        assert!(search_res.ok);
        let data = search_res.data.unwrap();
        let hits = data.get("results").unwrap().as_array().unwrap();
        assert!(
            !hits.is_empty(),
            "File watcher did not start for hot-loaded workspace"
        );

        // Clean shutdown
        send_req(&mut write_half, "shutdown", "shutdown", json!({})).await;
        let res = recv_res(&mut reader).await;
        assert!(res.ok, "res not ok: {:?}", res.error);
    }

    // The child daemon should exit cleanly after the IPC shutdown.
    let status = child
        .wait()
        .await
        .expect("failed to wait for daemon process");
    assert!(
        status.success(),
        "daemon process exited abnormally.\nstderr:\n{}",
        fs::read_to_string(&stderr_log).unwrap_or_default()
    );
}

#[tokio::test]
async fn test_scenario_4_daemon_lifecycle_recovery() {
    let temp = TempDir::new().unwrap();
    let canonical_path = std::fs::canonicalize(temp.path()).unwrap();
    let socket_path = canonical_path.join("daemon.sock");
    let pid_path = canonical_path.join("text-runtime.pid");

    let config = DaemonConfig {
        socket_path: socket_path.clone(),
        pid_path: pid_path.clone(),
        shutdown_grace_seconds: 1,
        workspaces: vec![],
    };

    // 1. Start daemon 1
    let daemon_task1 = tokio::spawn(text_runtime::daemon::run(config.clone()));

    // Wait for bind
    let mut attempts = 0;
    while !socket_path.exists() && attempts < 20 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        attempts += 1;
    }
    assert!(socket_path.exists());
    assert!(pid_path.exists());

    // 2. Try starting daemon 2 with the same config. It should fail and return DaemonAlreadyRunning
    let daemon_result2 = text_runtime::daemon::run(config.clone()).await;
    assert!(matches!(
        daemon_result2,
        Err(text_runtime::error::TextRuntimeError::DaemonAlreadyRunning)
    ));

    // 3. Connect to daemon 1, verify we can talk to it, then shut it down
    {
        let (read_half, mut write_half) = UnixStream::connect(&socket_path)
            .await
            .map(tokio::io::split)
            .unwrap();
        let mut reader = BufReader::new(read_half);

        send_req(&mut write_half, "req-1", "status", json!({})).await;
        let res = recv_res(&mut reader).await;
        assert!(res.ok, "res not ok: {:?}", res.error);

        send_req(&mut write_half, "shutdown", "shutdown", json!({})).await;
        let res = recv_res(&mut reader).await;
        assert!(res.ok, "res not ok: {:?}", res.error);
    }

    // 4. Verify the daemon task finishes, and unlinks socket and PID file
    daemon_task1.await.unwrap().unwrap();

    assert!(
        !socket_path.exists(),
        "Socket path was not unlinked on shutdown"
    );
    assert!(!pid_path.exists(), "PID file was not unlinked on shutdown");
}
