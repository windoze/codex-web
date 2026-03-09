use std::pin::Pin;

use crate::runners::{Runner, RunnerOutcome, RunnerTurnContext};
use crate::tool::ToolKind;

#[derive(Clone)]
pub struct ClaudeRunner {}

impl Runner for ClaudeRunner {
    fn tool(&self) -> ToolKind {
        ToolKind::ClaudeCode
    }

    fn run_turn<'a>(
        &'a self,
        _ctx: RunnerTurnContext,
    ) -> Pin<Box<dyn std::future::Future<Output = anyhow::Result<RunnerOutcome>> + Send + 'a>> {
        Box::pin(async move {
            anyhow::bail!(
                "claude-code runs are not implemented yet (tool selection is stored, but only Codex execution is supported)"
            )
        })
    }
}

