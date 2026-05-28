pub mod demod;
pub mod dsp;
pub mod handler;
pub mod http;
pub mod rtltcp;
pub mod session;
pub mod shared_sdr;

use std::sync::Arc;

use async_trait::async_trait;
use clap::{Arg, ArgMatches, Command, value_parser};
use tokio::io;
use tokio::task::JoinHandle;
use zako3_tap_sdk::tap;

use self::handler::SdrTapHandler;
use self::session::AudioTapSession;
use self::shared_sdr::SharedSdr;
use crate::modules::session::{EndSessionReason, Session};
use crate::modules::{HttpConfigurator, XngModule};

pub const MODULE_ID: &str = "audiotap";

pub struct AudioTapModule {
    tap_id: Option<String>,
    api_token: Option<String>,
    hub_endpoint: String,
    server_name: Option<String>,
    rtltcp_host: String,
    rtltcp_port: u16,
    center_hz: u32,
    healthcheck_port: Option<u16>,
    shared_sdr: Option<Arc<SharedSdr>>,
    sdr_task: Option<JoinHandle<()>>,
    zako_task: Option<JoinHandle<()>>,
}

impl AudioTapModule {
    pub fn new() -> Self {
        Self {
            tap_id: None,
            api_token: None,
            hub_endpoint: "api.zako.ac".to_string(),
            server_name: None,
            rtltcp_host: "localhost".to_string(),
            rtltcp_port: 1234,
            center_hz: 0,
            healthcheck_port: None,
            shared_sdr: None,
            sdr_task: None,
            zako_task: None,
        }
    }
}

#[async_trait]
impl XngModule for AudioTapModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }

    fn get_arguments(&self) -> Command {
        Command::new(MODULE_ID)
            .about("Live FM/AM SDR tap streaming Opus audio to the Zako3 Hub")
            .args(&[
                Arg::new("tap-id")
                    .long("tap-id")
                    .env("SDR_TAP_ID")
                    .required(true)
                    .help("Unique tap identifier registered with the Zako3 Hub"),
                Arg::new("api-token")
                    .long("api-token")
                    .env("SDR_API_TOKEN")
                    .required(true)
                    .help("Zako3 Hub API token"),
                Arg::new("center-mhz")
                    .long("center-mhz")
                    .env("SDR_CENTER_MHZ")
                    .required(true)
                    .value_parser(value_parser!(f64))
                    .help("SDR centre frequency in MHz (e.g. 98.0)"),
                Arg::new("hub")
                    .long("hub")
                    .env("TAPHUB_ENDPOINT")
                    .default_value("api.zako.ac")
                    .help("Zako3 Hub endpoint hostname"),
                Arg::new("server-name")
                    .long("server-name")
                    .env("TAPHUB_SERVER_NAME")
                    .help("TLS SNI override for the Hub connection"),
                Arg::new("rtltcp-host")
                    .long("rtltcp-host")
                    .env("RTLTCP_HOST")
                    .default_value("localhost")
                    .help("rtl_tcp server hostname"),
                Arg::new("rtltcp-port")
                    .long("rtltcp-port")
                    .env("RTLTCP_PORT")
                    .default_value("1234")
                    .value_parser(value_parser!(u16))
                    .help("rtl_tcp server port"),
                Arg::new("healthcheck-port")
                    .long("healthcheck-port")
                    .env("TAP_HEALTHCHECK_PORT")
                    .value_parser(value_parser!(u16))
                    .help("Optional standalone Zako3 SDK healthcheck port (separate from --listen-port)"),
            ])
    }

    fn parse_arguments(&mut self, args: &ArgMatches) -> Result<(), io::Error> {
        self.tap_id = Some(
            args.get_one::<String>("tap-id")
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "tap-id required"))?,
        );
        self.api_token = Some(
            args.get_one::<String>("api-token")
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "api-token required"))?,
        );
        let center_mhz = *args
            .get_one::<f64>("center-mhz")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "center-mhz required"))?;
        self.center_hz = (center_mhz * 1_000_000.0).round() as u32;
        if let Some(hub) = args.get_one::<String>("hub") {
            self.hub_endpoint = hub.clone();
        }
        self.server_name = args.get_one::<String>("server-name").cloned();
        if let Some(host) = args.get_one::<String>("rtltcp-host") {
            self.rtltcp_host = host.clone();
        }
        if let Some(port) = args.get_one::<u16>("rtltcp-port") {
            self.rtltcp_port = *port;
        }
        self.healthcheck_port = args.get_one::<u16>("healthcheck-port").copied();
        Ok(())
    }

    async fn init(&mut self) -> Result<(), io::Error> {
        let shared_sdr = SharedSdr::new(self.center_hz);
        self.shared_sdr = Some(shared_sdr.clone());

        let sdr_task = {
            let sdr = shared_sdr.clone();
            let host = self.rtltcp_host.clone();
            let port = self.rtltcp_port;
            tokio::spawn(async move {
                sdr.run(host, port).await;
            })
        };
        self.sdr_task = Some(sdr_task);

        let handler = SdrTapHandler {
            sdr: shared_sdr.clone(),
        };
        let mut builder = tap()
            .hub(&self.hub_endpoint)
            .transport(zako3_tap_sdk::Transport::Protofish3)
            .tap_id(self.tap_id.as_deref().unwrap_or(""))
            .friendly_name("SDR Live Tap")
            .api_token(self.api_token.as_deref().unwrap_or(""))
            .selection_weight(1.0);
        if let Some(ref sn) = self.server_name {
            builder = builder.server_name(sn);
        }
        if let Some(port) = self.healthcheck_port {
            builder = builder.healthcheck_port(port);
        }

        let zako_task = tokio::spawn(async move {
            if let Err(e) = builder.run(Arc::new(handler)).await {
                tracing::error!("Zako3 tap exited with error: {e:?}");
            } else {
                tracing::warn!("Zako3 tap exited cleanly");
            }
        });
        self.zako_task = Some(zako_task);

        Ok(())
    }

    async fn start_session(
        &mut self,
        _last_end_reason: EndSessionReason,
    ) -> Result<Box<dyn Session>, io::Error> {
        Ok(Box::new(AudioTapSession::new()))
    }

    fn http_configurator(&self) -> HttpConfigurator {
        let sdr = self
            .shared_sdr
            .clone()
            .expect("http_configurator called before init()");
        http::build_configurator(sdr)
    }

    async fn shutdown(&mut self) {
        if let Some(handle) = self.zako_task.take() {
            handle.abort();
        }
        if let Some(handle) = self.sdr_task.take() {
            handle.abort();
        }
    }
}

impl Default for AudioTapModule {
    fn default() -> Self {
        Self::new()
    }
}
