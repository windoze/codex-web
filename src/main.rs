use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = codex_web::config::Cli::parse();
    match cli.command {
        codex_web::config::Command::Serve(args) => {
            let config = codex_web::config::Config::from_serve_args(args)?;
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(codex_web::server::run(config))?;
        }
    }
    Ok(())
}
