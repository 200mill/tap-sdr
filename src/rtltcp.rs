use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct RtlTcpClient {
    stream: TcpStream,
}

impl RtlTcpClient {
    pub async fn connect(
        host: &str,
        port: u16,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut stream = TcpStream::connect((host, port)).await?;

        // Read and validate the 12-byte magic header: "RTL0" + tuner type (4) + gain count (4)
        let mut header = [0u8; 12];
        stream.read_exact(&mut header).await?;
        if &header[..4] != b"RTL0" {
            return Err(format!("unexpected rtl_tcp magic: {:?}", &header[..4]).into());
        }

        tracing::debug!(
            tuner_type = u32::from_be_bytes(header[4..8].try_into().unwrap()),
            gain_count = u32::from_be_bytes(header[8..12].try_into().unwrap()),
            "connected to rtl_tcp"
        );

        Ok(Self { stream })
    }

    async fn send_command(
        &mut self,
        cmd: u8,
        param: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut buf = [0u8; 5];
        buf[0] = cmd;
        buf[1..5].copy_from_slice(&param.to_be_bytes());
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    pub async fn set_frequency(
        &mut self,
        hz: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing::debug!(hz, "set frequency");
        self.send_command(0x01, hz).await
    }

    pub async fn set_sample_rate(
        &mut self,
        hz: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing::debug!(hz, "set sample rate");
        self.send_command(0x02, hz).await
    }

    pub async fn set_gain_mode(
        &mut self,
        manual: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing::debug!(manual, "set tuner gain mode");
        self.send_command(0x03, manual as u32).await
    }

    /// Set tuner gain. `tenths_db` is gain in tenths of a dB (e.g. 150 = 15.0 dB).
    pub async fn set_gain(
        &mut self,
        tenths_db: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing::debug!(gain_db = tenths_db as f32 / 10.0, "set tuner gain");
        self.send_command(0x04, tenths_db).await
    }

    pub async fn set_agc_mode(
        &mut self,
        on: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing::debug!(on, "set AGC mode");
        self.send_command(0x08, on as u32).await
    }

    /// Fill `buf` with raw interleaved I/Q bytes (u8, offset by 128).
    pub async fn read_samples(
        &mut self,
        buf: &mut [u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.stream.read_exact(buf).await?;
        Ok(())
    }
}
