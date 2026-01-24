/// RTP Packet Recorder
/// Captures raw UDP/RTP packets to a pcap file for replay testing
///
/// Requires the `record` feature and libpcap installed:
///   cargo build --features record
///   sudo ./target/release/phonecheck --once --record-pcap capture.pcap

#[cfg(feature = "record")]
use anyhow::{Context, Result};
#[cfg(feature = "record")]
use pcap::{Capture, Device, Savefile};
#[cfg(feature = "record")]
use std::path::Path;
#[cfg(feature = "record")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "record")]
use std::sync::Arc;
#[cfg(feature = "record")]
use tracing::{debug, info, warn};

#[cfg(feature = "record")]
pub struct RtpRecorder {
    savefile: Savefile,
    stop_flag: Arc<AtomicBool>,
    packet_count: u64,
}

#[cfg(feature = "record")]
impl RtpRecorder {
    /// Create a new recorder that captures RTP packets on the specified port range
    pub fn new<P: AsRef<Path>>(
        output_path: P,
        rtp_port_start: u16,
        rtp_port_end: u16,
    ) -> Result<Self> {
        // Find the default device
        let device = Device::lookup()
            .context("Failed to lookup network device")?
            .context("No network device found")?;

        info!("Capturing on device: {}", device.name);

        // Open capture
        let mut cap = Capture::from_device(device)
            .context("Failed to open capture device")?
            .promisc(false)
            .snaplen(2048) // Enough for RTP packets
            .timeout(100) // 100ms timeout for polling
            .open()
            .context("Failed to start capture")?;

        // Set BPF filter for UDP on our port range
        let filter = format!("udp portrange {}-{}", rtp_port_start, rtp_port_end);
        cap.filter(&filter, true)
            .context("Failed to set capture filter")?;

        info!("Capture filter: {}", filter);

        // Create savefile
        let savefile = cap
            .savefile(output_path.as_ref())
            .context("Failed to create pcap savefile")?;

        Ok(Self {
            savefile,
            stop_flag: Arc::new(AtomicBool::new(false)),
            packet_count: 0,
        })
    }

    /// Get a handle to stop recording from another thread
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop_flag)
    }

    /// Record packets until stop is called
    /// This should be run in a separate thread/task
    pub fn record_blocking(&mut self, mut cap: pcap::Capture<pcap::Active>) -> Result<u64> {
        info!("Recording started...");

        while !self.stop_flag.load(Ordering::Relaxed) {
            match cap.next_packet() {
                Ok(packet) => {
                    self.savefile.write(&packet);
                    self.packet_count += 1;
                    if self.packet_count % 100 == 0 {
                        debug!("Captured {} packets", self.packet_count);
                    }
                }
                Err(pcap::Error::TimeoutExpired) => {
                    // Normal timeout, continue polling
                    continue;
                }
                Err(e) => {
                    warn!("Capture error: {}", e);
                    break;
                }
            }
        }

        info!("Recording stopped. Total packets: {}", self.packet_count);
        Ok(self.packet_count)
    }

    /// Get the number of packets captured so far
    pub fn packet_count(&self) -> u64 {
        self.packet_count
    }
}

/// Standalone function to record RTP packets for a duration
#[cfg(feature = "record")]
pub fn record_rtp_to_file<P: AsRef<Path>>(
    output_path: P,
    rtp_port: u16,
    duration: std::time::Duration,
) -> Result<u64> {
    use std::thread;

    // Find the default device
    let device = Device::lookup()
        .context("Failed to lookup network device")?
        .context("No network device found")?;

    info!("Recording RTP on port {} to {:?}", rtp_port, output_path.as_ref());
    info!("Capturing on device: {}", device.name);

    // Open capture
    let mut cap = Capture::from_device(device)
        .context("Failed to open capture device")?
        .promisc(false)
        .snaplen(2048)
        .timeout(100)
        .open()
        .context("Failed to start capture")?;

    // Filter for our specific RTP port (both src and dst)
    let filter = format!("udp port {}", rtp_port);
    cap.filter(&filter, true)
        .context("Failed to set capture filter")?;

    // Create savefile
    let mut savefile = cap
        .savefile(output_path.as_ref())
        .context("Failed to create pcap savefile")?;

    let start = std::time::Instant::now();
    let mut packet_count = 0u64;

    while start.elapsed() < duration {
        match cap.next_packet() {
            Ok(packet) => {
                savefile.write(&packet);
                packet_count += 1;
            }
            Err(pcap::Error::TimeoutExpired) => continue,
            Err(e) => {
                warn!("Capture error: {}", e);
                break;
            }
        }
    }

    // Ensure data is flushed
    drop(savefile);

    info!("Recorded {} packets to {:?}", packet_count, output_path.as_ref());
    Ok(packet_count)
}

#[cfg(not(feature = "record"))]
pub fn record_rtp_to_file<P: AsRef<std::path::Path>>(
    _output_path: P,
    _rtp_port: u16,
    _duration: std::time::Duration,
) -> anyhow::Result<u64> {
    anyhow::bail!("Recording requires the 'record' feature. Build with: cargo build --features record")
}
