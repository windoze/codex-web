use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use codex_web::codex::CodexRuntime;
use codex_web::db::{Db, RunStatus};
use codex_web::runners::{Runner, RunnerTurnContext};
use codex_web::runners::codex::CodexRunner;
use codex_web::tool::ToolKind;

#[tokio::test]
async fn ssh_project_stub_turn_produces_agent_message() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("ssh_stub_turn.sqlite3");
    let db = Db::connect(&db_path).await.expect("db");

    // Create an SSH project directly in the database.
    let project = db
        .create_ssh_project(
            "ssh-test",
            "user@remote",
            Some(2222),
            "/home/user/repo",
            None,
            None,
        )
        .await
        .expect("create ssh project");

    assert_eq!(project.kind, codex_web::db::ProjectKind::Ssh);
    assert_eq!(project.ssh_target.as_deref(), Some("user@remote"));
    assert_eq!(project.ssh_port, Some(2222));
    assert_eq!(
        project.remote_root_path.as_deref(),
        Some("/home/user/repo")
    );

    let convo = db
        .create_conversation(Some(project.id), "ssh-convo", ToolKind::Codex)
        .await
        .expect("create conversation");
    db.try_mark_run_running(convo.id)
        .await
        .expect("mark running");

    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(64);

    // Use stub runtime (doesn't actually SSH), just verifies the flow.
    let stub_events = vec![serde_json::json!({
        "type": "item_completed",
        "thread_id": "00000000-0000-0000-0000-000000000001",
        "turn_id": "turn_0",
        "item": {
            "type": "AgentMessage",
            "id": "item_0",
            "content": [
                { "type": "Text", "text": "hello from ssh stub" }
            ]
        }
    })];

    let runner = CodexRunner {
        runtime: CodexRuntime::stub(stub_events),
    };

    let outcome = runner
        .run_turn(RunnerTurnContext {
            db: db.clone(),
            event_tx: event_tx.clone(),
            conversation_id: convo.id,
            project_root: temp_dir.path().to_path_buf(),
            project: project.clone(),
            tool_session_id: None,
            prompt: "test ssh".to_string(),
            ws_clients: Arc::new(AtomicUsize::new(0)),
            interaction_timeout_ms: 30_000,
            interaction_default_action: "decline".to_string(),
        })
        .await
        .expect("run turn");

    // Should have a session id from the stub event.
    assert!(outcome.tool_session_id.is_some());

    // Should see an agent_message event broadcast.
    let mut saw_agent = false;
    for _ in 0..10 {
        if let Ok(e) =
            tokio::time::timeout(std::time::Duration::from_millis(200), event_rx.recv()).await
        {
            let e = e.expect("recv");
            if e.event_type == "agent_message" {
                saw_agent = true;
                break;
            }
        }
    }
    assert!(saw_agent, "expected agent_message event from SSH stub");

    let run = db.get_run(convo.id).await.expect("get run");
    assert_eq!(run.status, RunStatus::Running);
}

#[tokio::test]
async fn ssh_project_db_fields_roundtrip() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("ssh_fields_roundtrip.sqlite3");
    let db = Db::connect(&db_path).await.expect("db");

    let project = db
        .create_ssh_project(
            "full-ssh",
            "admin@10.0.0.1",
            Some(22),
            "/opt/project",
            Some("/home/me/.ssh/id_ed25519"),
            Some("strict"),
        )
        .await
        .expect("create ssh project");

    // Verify all fields survived the roundtrip.
    assert_eq!(project.kind, codex_web::db::ProjectKind::Ssh);
    assert_eq!(project.ssh_target.as_deref(), Some("admin@10.0.0.1"));
    assert_eq!(project.ssh_port, Some(22));
    assert_eq!(project.remote_root_path.as_deref(), Some("/opt/project"));
    assert_eq!(
        project.ssh_identity_file.as_deref(),
        Some("/home/me/.ssh/id_ed25519")
    );
    assert_eq!(
        project.ssh_known_hosts_policy.as_deref(),
        Some("strict")
    );

    // Fetch it again via list_projects to make sure DB persistence works.
    let projects = db.list_projects().await.expect("list projects");
    let found = projects
        .iter()
        .find(|p| p.id == project.id)
        .expect("project in list");
    assert_eq!(found.kind, codex_web::db::ProjectKind::Ssh);
    assert_eq!(found.ssh_target.as_deref(), Some("admin@10.0.0.1"));
    assert_eq!(found.ssh_port, Some(22));
}
