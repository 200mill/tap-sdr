// Vendored and adapted from airframesio/xng (src/modules/session.rs, GPL-3.0-or-later).
// Audio-stream lifecycle uses a degenerate Session whose read_message blocks forever
// — the orchestrator drives shutdown via interrupt and CancellationToken.

use async_trait::async_trait;
use tokio::io;

#[derive(Copy, Clone, Debug)]
pub enum EndSessionReason {
    None,
    UserInterrupt,
    ReadError,
}

#[async_trait]
pub trait Session: Send {
    /// Block until either a message arrives or the underlying source errors.
    /// For audio-stream modules, this is intentionally a forever-pending future;
    /// the orchestrator races it against the interrupt signal.
    async fn read_message(&mut self, msg: &mut String) -> Result<usize, io::Error>;

    async fn end(&mut self, reason: EndSessionReason);
}
