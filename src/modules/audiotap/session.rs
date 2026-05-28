use async_trait::async_trait;
use std::future::pending;
use tokio::io;

use crate::modules::session::{EndSessionReason, Session};

/// Degenerate Session: the audio pipeline is line-free and runs entirely under
/// tasks spawned in `AudioTapModule::init`. The orchestrator never expects a
/// "message" from us, so we block forever and let the interrupt path drive shutdown.
pub struct AudioTapSession {}

impl AudioTapSession {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for AudioTapSession {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Session for AudioTapSession {
    async fn read_message(&mut self, _msg: &mut String) -> Result<usize, io::Error> {
        pending::<()>().await;
        unreachable!()
    }

    async fn end(&mut self, _reason: EndSessionReason) {}
}
