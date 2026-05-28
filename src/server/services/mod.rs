pub mod health;

use actix_web::web::{self, ServiceConfig};

pub fn config(cfg: &mut ServiceConfig) {
    cfg.route("/healthz", web::get().to(health::healthz));
}
