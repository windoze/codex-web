//! Smoke test for the native `claude` binary integration.
//!
//! This is ignored by default because it requires:
//! - `claude` installed and authenticated on the machine running the test
//! - network access and a real model invocation (may incur cost)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use codex_web::claude::{ClaudeInvocation, ClaudeOutputLine, ClaudeRuntime, run_jsonl_events_with_input};

#[tokio::test]
#[ignore]
async fn native_claude_emits_assistant_text() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;

    // Best-effort guard: if `claude` isn't available, skip.
    if std::process::Command::new("claude")
        .arg("--version")
        .output()
        .is_err()
    {
        return Ok(());
    }

    let saw_text = Arc::new(AtomicBool::new(false));
    let saw_text_for_cb = saw_text.clone();

    let outcome = run_jsonl_events_with_input(
        ClaudeRuntime::real("claude".to_string(), vec![]),
        ClaudeInvocation {
            project_root: temp_dir.path().to_path_buf(),
            session_id: None,
            prompt: "hello".to_string(),
        },
        move |line| {
            let saw_text = saw_text_for_cb.clone();
            async move {
                if let ClaudeOutputLine::Json(v) = line {
                    let typ = v.get("type").and_then(|t| t.as_str());
                    match typ {
                        Some("assistant_message_delta") => {
                            if v.get("delta").and_then(|d| d.as_str()).is_some() {
                                saw_text.store(true, Ordering::Relaxed);
                            }
                        }
                        Some("assistant_message_completed") => {
                            if v.get("text").and_then(|t| t.as_str()).is_some() {
                                saw_text.store(true, Ordering::Relaxed);
                            }
                        }
                        _ => {}
                    }
                }
                Ok::<(), anyhow::Error>(())
            }
        },
        |_event| async { Ok(None) },
    )
    .await?;

    assert!(
        saw_text.load(Ordering::Relaxed),
        "expected native claude stream to include assistant text"
    );
    assert!(
        outcome.session_id.is_some(),
        "expected a session id to be produced"
    );

    Ok(())
}

