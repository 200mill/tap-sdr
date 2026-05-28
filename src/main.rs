use clap::command;
use std::process::exit;
use tokio::runtime::Runtime;

mod common;
mod modules;
mod server;

fn main() {
    dotenvy::dotenv().ok();
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    tracing_subscriber::fmt::init();

    let mut manager = modules::ModuleManager::init();

    let cmd = manager.register_arguments(
        command!()
            .propagate_version(true)
            .subcommand_required(true)
            .arg_required_else_help(true),
    );

    let args = cmd.get_matches();

    let rt = match Runtime::new() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Failed to start tokio: {}", e);
            exit(exitcode::OSERR);
        }
    };

    rt.block_on(async {
        match args.subcommand() {
            Some((subcmd, matches)) => manager.start(subcmd, matches).await,
            None => unreachable!("subcommand_required is true"),
        }
    });
}
