// Vendored and adapted from airframesio/xng (src/modules/mod.rs, GPL-3.0-or-later).
// Trimmed to: clap subcommand routing, actix-web HTTP server, SIGINT/SIGTERM shutdown.
// Removed: CommonFrame/ES/swarm/airframes.io pipelines, StateDB, ModuleSettings, reload signal.

use actix_web::web::ServiceConfig;
use actix_web::{App, HttpServer, middleware};
use async_trait::async_trait;
use clap::{ArgMatches, Command};
use std::collections::HashMap;
use std::process::exit;
use std::sync::Arc;
use tokio::io;
use tokio::select;
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;

use crate::common;
use crate::common::arguments::{parse_disable_cross_site, parse_listen_host, parse_listen_port};
use crate::server::services as server_services;

pub mod audiotap;
pub mod session;

use self::session::{EndSessionReason, Session};

const DEFAULT_LISTEN_HOST: &str = "127.0.0.1";
const DEFAULT_LISTEN_PORT: u16 = 7871;

pub type HttpConfigurator = Box<dyn Fn(&mut ServiceConfig) + Send + Sync>;

/// Per-module surface used by the orchestrator. Each module is a long-running
/// SDR pipeline that publishes its own HTTP routes for control + observability.
#[async_trait]
pub trait XngModule: Send + Sync {
    fn id(&self) -> &'static str;
    fn get_arguments(&self) -> Command;
    fn parse_arguments(&mut self, args: &ArgMatches) -> Result<(), io::Error>;
    async fn init(&mut self) -> Result<(), io::Error>;
    async fn start_session(
        &mut self,
        last_end_reason: EndSessionReason,
    ) -> Result<Box<dyn Session>, io::Error>;

    /// Returns a closure that installs this module's HTTP routes into a fresh actix App.
    /// Called once per worker; must be cheap and idempotent.
    fn http_configurator(&self) -> HttpConfigurator;

    async fn shutdown(&mut self);
}

pub struct ModuleManager {
    modules: HashMap<&'static str, Box<dyn XngModule>>,
}

impl ModuleManager {
    pub fn init() -> ModuleManager {
        let modules: Vec<(&'static str, Box<dyn XngModule>)> = vec![{
            let m: Box<dyn XngModule> = Box::new(audiotap::AudioTapModule::new());
            (m.id(), m)
        }];
        ModuleManager {
            modules: HashMap::from_iter(modules),
        }
    }

    pub fn register_arguments(&self, cmd: Command) -> Command {
        cmd.subcommands(
            self.modules
                .values()
                .map(|m| common::arguments::register_common_arguments(m.get_arguments()))
                .collect::<Vec<Command>>(),
        )
    }

    pub async fn start(&mut self, cmd: &str, args: &ArgMatches) {
        let Some(module) = self.modules.get_mut(cmd) else {
            tracing::error!("Invalid module '{}', please choose a valid module.", cmd);
            exit(exitcode::CONFIG);
        };

        if let Err(e) = module.parse_arguments(args) {
            tracing::error!("Failed to parse arguments: {}", e);
            return;
        }

        let listen_host = parse_listen_host(args, DEFAULT_LISTEN_HOST);
        let listen_port = parse_listen_port(args, DEFAULT_LISTEN_PORT);
        let disable_cross_site = parse_disable_cross_site(args);

        if let Err(e) = module.init().await {
            tracing::error!("Failed to init module {}: {}", module.id(), e);
            return;
        }

        let configurator: Arc<HttpConfigurator> = Arc::new(module.http_configurator());

        let cancel_token = CancellationToken::new();
        let http_cancel_token = cancel_token.clone();

        let bind = (listen_host.clone(), listen_port);
        let restricted_origin = format!("http://{}:{}", listen_host, listen_port);
        let configurator_for_server = configurator.clone();

        let http_thread = tokio::spawn(async move {
            let server = HttpServer::new(move || {
                let c = configurator_for_server.clone();
                App::new()
                    .wrap(middleware::DefaultHeaders::new().add((
                        "Access-Control-Allow-Origin",
                        if disable_cross_site {
                            restricted_origin.clone()
                        } else {
                            "*".to_string()
                        },
                    )))
                    .configure(move |cfg| (c)(cfg))
                    .configure(server_services::config)
            })
            .bind(bind)
            .unwrap()
            .run();

            tracing::info!(
                "HTTP server started on http://{}:{}",
                listen_host,
                listen_port
            );

            select! {
                _ = server => {},
                _ = http_cancel_token.cancelled() => {
                    tracing::info!("HTTP server got cancel request");
                }
            }
        });

        let mut interrupt = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to register SIGINT handler: {}", e);
                return;
            }
        };
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to register SIGTERM handler: {}", e);
                return;
            }
        };

        let mut reason = EndSessionReason::None;
        let mut session = match module.start_session(reason).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("Failed to start session: {}", e);
                cancel_token.cancel();
                #[allow(unused_must_use)]
                {
                    http_thread.await;
                }
                return;
            }
        };

        loop {
            let mut raw_msg = String::new();
            select! {
                result = session.read_message(&mut raw_msg) => {
                    match result {
                        Ok(_) => continue,
                        Err(e) => {
                            tracing::error!("Session read error: {}", e);
                            reason = EndSessionReason::ReadError;
                            break;
                        }
                    }
                }
                _ = interrupt.recv() => {
                    tracing::warn!("Got SIGINT, exiting session cleanly...");
                    reason = EndSessionReason::UserInterrupt;
                    break;
                }
                _ = term.recv() => {
                    tracing::warn!("Got SIGTERM, exiting session cleanly...");
                    reason = EndSessionReason::UserInterrupt;
                    break;
                }
            }
        }

        session.end(reason).await;
        module.shutdown().await;
        cancel_token.cancel();

        #[allow(unused_must_use)]
        {
            http_thread.await;
        }
        tracing::info!("Exiting...");
    }
}
