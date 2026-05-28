use std::sync::Arc;

use actix_web::web::{self, Data, ServiceConfig};
use actix_web::{HttpResponse, Responder};
use serde::{Deserialize, Serialize};

use super::shared_sdr::SharedSdr;
use crate::modules::HttpConfigurator;

#[derive(Serialize)]
struct StatsResponse {
    center_hz: u32,
    sample_rate: u32,
    listeners: usize,
    chunks_pushed: u64,
    retunes: u64,
    gain_tenths_db: u32,
    hub_connected: bool,
}

#[derive(Deserialize)]
struct RetuneRequest {
    hz: Option<u32>,
    mhz: Option<f64>,
}

#[derive(Serialize)]
struct RetuneResponse {
    actual_hz: u32,
}

#[derive(Deserialize)]
struct GainRequest {
    tenths_db: u32,
}

async fn get_stats(sdr: Data<Arc<SharedSdr>>) -> impl Responder {
    HttpResponse::Ok().json(StatsResponse {
        center_hz: sdr.center_hz(),
        sample_rate: sdr.sample_rate,
        listeners: sdr.listener_count(),
        chunks_pushed: sdr.chunks_pushed(),
        retunes: sdr.retunes(),
        gain_tenths_db: sdr.current_gain_tenths_db(),
        hub_connected: sdr.hub_connected(),
    })
}

async fn post_retune(
    sdr: Data<Arc<SharedSdr>>,
    body: web::Json<RetuneRequest>,
) -> impl Responder {
    let target_hz = match (body.hz, body.mhz) {
        (Some(hz), _) => hz,
        (None, Some(mhz)) => (mhz * 1_000_000.0).round() as u32,
        (None, None) => {
            return HttpResponse::BadRequest().body("must provide hz or mhz");
        }
    };
    // RTL-SDR R820T tunes ~24 MHz to ~1.766 GHz; reject obviously out-of-range values.
    if !(24_000_000..=1_800_000_000).contains(&target_hz) {
        return HttpResponse::BadRequest().body("target frequency out of RTL-SDR range");
    }
    sdr.retune(target_hz).await;
    HttpResponse::Ok().json(RetuneResponse {
        actual_hz: sdr.center_hz(),
    })
}

async fn post_gain(
    sdr: Data<Arc<SharedSdr>>,
    body: web::Json<GainRequest>,
) -> impl Responder {
    // Most R820T-class tuners top out around 49.6 dB.
    if body.tenths_db > 500 {
        return HttpResponse::BadRequest().body("tenths_db out of plausible range (max 500)");
    }
    sdr.set_gain(body.tenths_db);
    HttpResponse::NoContent().finish()
}

pub fn build_configurator(sdr: Arc<SharedSdr>) -> HttpConfigurator {
    Box::new(move |cfg: &mut ServiceConfig| {
        cfg.app_data(Data::new(sdr.clone()))
            .route("/api/v1/stats", web::get().to(get_stats))
            .route("/api/v1/retune", web::post().to(post_retune))
            .route("/api/v1/gain", web::post().to(post_gain));
    })
}
