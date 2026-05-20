use std::sync::Arc;
use zako3_tap_sdk::tap;

pub mod demod;
pub mod rtltcp;
pub mod sdr;
pub mod sdr_source;
pub mod shared_sdr;
#[cfg(feature = "soapy")]
pub mod soapysdr_source;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    tracing_subscriber::fmt::init();

    let tap_id = std::env::var("SDR_TAP_ID").expect("SDR_TAP_ID is required");
    let api_token = std::env::var("SDR_API_TOKEN").expect("SDR_API_TOKEN is required");
    let hub = std::env::var("TAPHUB_ENDPOINT").unwrap_or_else(|_| "api.zako.ac".to_string());
    let server_name = std::env::var("TAPHUB_SERVER_NAME").ok();
    let healthcheck_port = std::env::var("TAP_HEALTHCHECK_PORT").ok().map(|v| {
        v.parse::<u16>()
            .expect("TAP_HEALTHCHECK_PORT must be a valid port number")
    });
    let center_hz = std::env::var("SDR_CENTER_MHZ")
        .map(|v| {
            (v.parse::<f64>().expect("SDR_CENTER_MHZ must be a number") * 1_000_000.0).round()
                as u32
        })
        .expect("SDR_CENTER_MHZ is required (e.g. SDR_CENTER_MHZ=98.0)");
    let gain_db: f32 = std::env::var("SDR_GAIN_DB")
        .unwrap_or_else(|_| "15.0".to_string())
        .parse()
        .expect("SDR_GAIN_DB must be a number");
    let driver = std::env::var("SDR_DRIVER").unwrap_or_else(|_| "rtltcp".to_string());

    let source: Box<dyn sdr_source::SdrSource> = match driver.as_str() {
        "rtltcp" => {
            let host =
                std::env::var("RTLTCP_HOST").unwrap_or_else(|_| "localhost".to_string());
            let port = std::env::var("RTLTCP_PORT")
                .unwrap_or_else(|_| "1234".to_string())
                .parse::<u16>()
                .expect("RTLTCP_PORT must be a valid port number");
            Box::new(rtltcp::RtlTcpSource::new(host, port))
        }
        #[cfg(feature = "soapy")]
        "soapy" => {
            let args =
                std::env::var("SOAPY_ARGS").expect("SOAPY_ARGS is required for SDR_DRIVER=soapy");
            Box::new(soapysdr_source::SoapySdrSource::new(args))
        }
        other => panic!("unknown SDR_DRIVER: {other:?} (supported: rtltcp, soapy)"),
    };

    let shared_sdr = shared_sdr::SharedSdr::new(center_hz, gain_db);

    let sdr_task = Arc::clone(&shared_sdr);
    tokio::spawn(async move {
        sdr_task.run(source).await;
    });

    let handler = sdr::SdrTapHandler { sdr: shared_sdr };

    let mut builder = tap()
        .hub(&hub)
        .tap_id(&tap_id)
        .friendly_name("SDR Live Tap")
        .api_token(&api_token)
        .selection_weight(1.0);

    if let Some(ref sn) = server_name {
        builder = builder.server_name(sn);
    }
    if let Some(port) = healthcheck_port {
        builder = builder.healthcheck_port(port);
    }

    builder.run(Arc::new(handler)).await?;

    Ok(())
}
