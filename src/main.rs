use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = codex_web::config::Cli::parse();
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        match cli.command {
            codex_web::config::Command::Serve(args) => {
                let config = codex_web::config::Config::from_serve_args(args)?;
                codex_web::server::run(config).await?;
            }
            codex_web::config::Command::Interactions(args) => {
                run_interactions_cli(args).await?;
            }
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

async fn run_interactions_cli(args: codex_web::config::InteractionsArgs) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = args.auth_token.as_deref().filter(|t| !t.trim().is_empty());

    match args.command {
        codex_web::config::InteractionsCommand::List { conversation_id } => {
            let url = match conversation_id {
                Some(id) => format!("{}/api/conversations/{}/interactions", args.daemon, id),
                None => format!("{}/api/interactions/pending", args.daemon),
            };
            let mut req = client.get(url);
            if let Some(token) = token {
                req = req.bearer_auth(token);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("request failed: {}: {}", status, body);
            }
            let json: serde_json::Value = serde_json::from_str(&body)?;
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        codex_web::config::InteractionsCommand::Respond {
            interaction_id,
            action,
            text,
        } => {
            let url = format!(
                "{}/api/interactions/{}/respond",
                args.daemon, interaction_id
            );
            let mut req = client
                .post(url)
                .json(&serde_json::json!({ "action": action, "text": text }));
            if let Some(token) = token {
                req = req.bearer_auth(token);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("request failed: {}: {}", status, body);
            }
            println!("{body}");
        }
    }

    Ok(())
}
