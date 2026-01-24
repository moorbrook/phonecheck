/// SIP Client - Makes outbound calls and manages call state
/// Implements basic SIP UAC (User Agent Client) functionality
///
/// NOTE: This implementation does not include SIP digest authentication.
/// The SIP trunk must be configured for IP-based authentication (allow calls
/// from the client's IP address without credentials). Most SIP providers
/// support this via "IP Auth" or "IP Whitelist" settings.
///
/// TODO: Implement RFC 2617 digest authentication for providers that require it.

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::lookup_host;
use tracing::{debug, info, warn};

use super::messages::{
    build_ack, build_bye, build_invite, extract_to_tag, extract_via_branch, generate_call_id,
    generate_tag, parse_status_code,
};
use super::transport::SipTransport;
use crate::config::Config;
use crate::rtp::RtpReceiver;

/// SIP client for making outbound calls
pub struct SipClient {
    config: Config,
    server_addr: SocketAddr,
}

/// Result of a phone check call
#[derive(Debug)]
pub struct CallResult {
    pub connected: bool,
    pub audio_received: bool,
    pub audio_samples: Vec<f32>,
    pub error: Option<String>,
}

impl SipClient {
    /// Create a new SIP client
    pub async fn new(config: Config) -> Result<Self> {
        // Resolve SIP server address
        let server_host = format!("{}:{}", config.sip_server, config.sip_port);
        let server_addr = lookup_host(&server_host)
            .await
            .context(format!("Failed to resolve SIP server: {}", server_host))?
            .next()
            .context("No addresses found for SIP server")?;

        info!("SIP server resolved to {}", server_addr);

        Ok(Self {
            config,
            server_addr,
        })
    }

    /// Make a test call and capture audio
    pub async fn make_test_call(&self, listen_duration: Duration) -> Result<CallResult> {
        // Set up RTP receiver first to get the port
        let mut rtp_receiver = RtpReceiver::bind(0).await?;
        let rtp_port = rtp_receiver.local_port()?;
        debug!("RTP receiver ready on port {}", rtp_port);

        // Create SIP transport
        let transport = SipTransport::new(self.server_addr).await?;
        let local_addr = transport.local_addr()?;

        // Generate call identifiers
        let call_id = generate_call_id(&local_addr.ip().to_string());
        let from_tag = generate_tag();
        let from_uri = format!(
            "sip:{}@{}",
            self.config.sip_username, self.config.sip_server
        );
        let target_uri = format!("sip:{}@{}", self.config.target_phone, self.config.sip_server);

        info!("Initiating call to {}", self.config.target_phone);

        // Build and send INVITE
        let invite = build_invite(
            &target_uri,
            &from_uri,
            "PhoneCheck",
            &call_id,
            &from_tag,
            1,
            local_addr,
            rtp_port,
        );

        transport.send(&invite).await?;

        // Wait for final response (with retries for provisional responses)
        let response = match transport
            .receive_final_response(Duration::from_secs(30), 20)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(CallResult {
                    connected: false,
                    audio_received: false,
                    audio_samples: Vec::new(),
                    error: Some(format!("No response from server: {}", e)),
                });
            }
        };

        let status_code = parse_status_code(&response).unwrap_or(0);
        debug!("Received final response: {}", status_code);

        if status_code != 200 {
            return Ok(CallResult {
                connected: false,
                audio_received: false,
                audio_samples: Vec::new(),
                error: Some(format!("Call rejected with status {}", status_code)),
            });
        }

        // Extract To tag for dialog
        let to_tag = extract_to_tag(&response);
        let via_branch = extract_via_branch(&response).unwrap_or_else(|| "z9hG4bKunknown".to_string());

        // Send ACK
        let ack = build_ack(
            &target_uri,
            &from_uri,
            "PhoneCheck",
            &target_uri,
            to_tag.as_deref(),
            &call_id,
            &from_tag,
            1,
            local_addr,
            &via_branch,
        );
        transport.send(&ack).await?;
        info!("Call connected, listening for audio...");

        // Receive RTP audio for specified duration
        rtp_receiver.receive_for(listen_duration).await?;
        let audio_samples = rtp_receiver.get_samples_f32();
        let audio_received = !audio_samples.is_empty();

        info!(
            "Received {} audio samples ({:.1}s)",
            audio_samples.len(),
            audio_samples.len() as f32 / 16000.0
        );

        // Send BYE to end call
        let bye = build_bye(
            &target_uri,
            &from_uri,
            "PhoneCheck",
            &target_uri,
            to_tag.as_deref(),
            &call_id,
            &from_tag,
            2,
            local_addr,
        );
        transport.send(&bye).await?;

        // Wait for BYE response (best effort, don't fail if not received)
        match transport.receive(Duration::from_secs(5)).await {
            Ok(bye_response) => {
                let bye_status = parse_status_code(&bye_response).unwrap_or(0);
                debug!("BYE response: {}", bye_status);
            }
            Err(e) => {
                warn!("No response to BYE: {}", e);
            }
        }

        info!("Call ended");

        Ok(CallResult {
            connected: true,
            audio_received,
            audio_samples,
            error: None,
        })
    }
}
