/// SIP Client - Makes outbound calls and manages call state
/// Implements basic SIP UAC (User Agent Client) functionality
///
/// Supports both IP-based authentication and RFC 2617/7616 digest authentication.
/// When the server responds with 401 or 407, the client will retry with
/// digest authentication using the configured SIP password.

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::lookup_host;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::digest::{extract_authenticate_header, DigestChallenge, DigestResponse};
use super::messages::{
    build_ack, build_bye, build_invite, build_invite_with_auth, extract_to_tag,
    extract_via_branch, generate_call_id, generate_tag, parse_status_code,
};
use super::transport::SipTransport;
use crate::config::Config;
use crate::rtp::RtpReceiver;

/// SIP client for making outbound calls
pub struct SipClient {
    config: Config,
    server_addr: SocketAddr,
    /// Public address discovered via STUN (if configured)
    public_addr: Option<std::net::IpAddr>,
}

/// Result of a phone check call
#[derive(Debug)]
pub struct CallResult {
    pub connected: bool,
    pub audio_received: bool,
    pub audio_samples: Vec<f32>,
    pub error: Option<String>,
    /// SIP status code if call was rejected
    pub sip_status: Option<u16>,
}

/// Classify SIP error codes for better error handling and reporting
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SipErrorCategory {
    /// 401/407: Authentication required
    AuthRequired,
    /// 404: Number not found / bad configuration
    NotFound,
    /// 486/600/603: Busy or declined
    Busy,
    /// 408/480/504: Timeout / unavailable
    Timeout,
    /// 500-599: Server error
    ServerError,
    /// 4xx other: Client error
    ClientError,
    /// 3xx: Redirect (not currently handled)
    Redirect,
    /// Unknown or unparseable
    Unknown,
}

impl SipErrorCategory {
    /// Classify a SIP status code
    pub fn from_status(code: u16) -> Self {
        match code {
            401 | 407 => SipErrorCategory::AuthRequired,
            404 => SipErrorCategory::NotFound,
            486 | 600 | 603 => SipErrorCategory::Busy,
            408 | 480 | 504 => SipErrorCategory::Timeout,
            500..=599 => SipErrorCategory::ServerError,
            400..=499 => SipErrorCategory::ClientError,
            300..=399 => SipErrorCategory::Redirect,
            _ => SipErrorCategory::Unknown,
        }
    }

    /// Human-readable description of the error
    pub fn description(&self) -> &'static str {
        match self {
            SipErrorCategory::AuthRequired => "SIP authentication required - configure IP authentication or implement digest auth",
            SipErrorCategory::NotFound => "Number not found - check TARGET_PHONE configuration",
            SipErrorCategory::Busy => "Line busy or call declined - try again later",
            SipErrorCategory::Timeout => "Call timeout - remote party unavailable",
            SipErrorCategory::ServerError => "SIP server error - try again later",
            SipErrorCategory::ClientError => "SIP client error - check configuration",
            SipErrorCategory::Redirect => "Call redirected - not currently supported",
            SipErrorCategory::Unknown => "Unknown SIP error",
        }
    }

    /// Whether this error type is transient (worth retrying)
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            SipErrorCategory::Busy
                | SipErrorCategory::Timeout
                | SipErrorCategory::ServerError
        )
    }
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

        // Discover public IP via STUN if configured
        let public_addr = if let Some(ref stun_server) = config.stun_server {
            match crate::stun::discover_public_address(stun_server).await {
                Ok(addr) => {
                    info!("STUN discovered public IP: {}", addr.ip());
                    Some(addr.ip())
                }
                Err(e) => {
                    warn!("STUN discovery failed, using local IP: {}", e);
                    None
                }
            }
        } else {
            debug!("No STUN server configured, using local IP");
            None
        };

        Ok(Self {
            config,
            server_addr,
            public_addr,
        })
    }

    /// Make a test call and capture audio
    pub async fn make_test_call(&self, listen_duration: Duration) -> Result<CallResult> {
        self.make_test_call_on_port(listen_duration, 0).await
    }

    /// Make a test call with cancellation support for graceful shutdown
    pub async fn make_test_call_cancellable(
        &self,
        listen_duration: Duration,
        cancel_token: CancellationToken,
    ) -> Result<CallResult> {
        self.make_test_call_on_port_cancellable(listen_duration, 0, cancel_token)
            .await
    }

    /// Make a test call on a specific RTP port (0 = auto-assign)
    /// Use this when you need to know the port in advance (e.g., for packet capture)
    pub async fn make_test_call_on_port(
        &self,
        listen_duration: Duration,
        rtp_port_hint: u16,
    ) -> Result<CallResult> {
        // No cancellation - create a dummy token that never cancels
        let cancel_token = CancellationToken::new();
        self.make_test_call_on_port_cancellable(listen_duration, rtp_port_hint, cancel_token)
            .await
    }

    /// Make a test call on a specific RTP port with cancellation support
    pub async fn make_test_call_on_port_cancellable(
        &self,
        listen_duration: Duration,
        rtp_port_hint: u16,
        cancel_token: CancellationToken,
    ) -> Result<CallResult> {
        // Set up RTP receiver on specified port (0 = auto-assign)
        let mut rtp_receiver = RtpReceiver::bind(rtp_port_hint).await?;
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

        // Build external RTP address for SDP (if STUN discovered public IP)
        let external_rtp_addr = self.public_addr.map(|ip| {
            std::net::SocketAddr::new(ip, rtp_port)
        });

        // Build and send initial INVITE
        let mut cseq = 1u32;
        let invite = build_invite(
            &target_uri,
            &from_uri,
            "PhoneCheck",
            &call_id,
            &from_tag,
            cseq,
            local_addr,
            rtp_port,
            external_rtp_addr,
        );

        // Send INVITE with RFC 3261 retransmission and wait for final response
        let mut response = match transport.send_invite_await_final(&invite).await {
            Ok(r) => r,
            Err(e) => {
                return Ok(CallResult {
                    connected: false,
                    audio_received: false,
                    audio_samples: Vec::new(),
                    error: Some(format!("No response from server: {}", e)),
                    sip_status: None,
                });
            }
        };

        let mut status_code = parse_status_code(&response).unwrap_or(0);
        debug!("Received final response: {}", status_code);

        // Handle 401/407 authentication challenge
        if status_code == 401 || status_code == 407 {
            // Check if we have a password configured
            if self.config.sip_password.is_empty() {
                let error_msg = format!(
                    "SIP server requires authentication ({}), but no SIP_PASSWORD configured",
                    status_code
                );
                tracing::error!("{}", error_msg);
                return Ok(CallResult {
                    connected: false,
                    audio_received: false,
                    audio_samples: Vec::new(),
                    error: Some(error_msg),
                    sip_status: Some(status_code),
                });
            }

            // Extract and parse the challenge
            let auth_header = match extract_authenticate_header(&response) {
                Some(h) => h,
                None => {
                    let error_msg = "Server sent 401/407 but no WWW-Authenticate header found";
                    tracing::error!("{}", error_msg);
                    return Ok(CallResult {
                        connected: false,
                        audio_received: false,
                        audio_samples: Vec::new(),
                        error: Some(error_msg.to_string()),
                        sip_status: Some(status_code),
                    });
                }
            };

            let challenge = match DigestChallenge::parse(&auth_header) {
                Some(c) => c,
                None => {
                    let error_msg = format!(
                        "Failed to parse authentication challenge: {}",
                        auth_header
                    );
                    tracing::error!("{}", error_msg);
                    return Ok(CallResult {
                        connected: false,
                        audio_received: false,
                        audio_samples: Vec::new(),
                        error: Some(error_msg),
                        sip_status: Some(status_code),
                    });
                }
            };

            info!(
                "Received {} challenge (realm: {}), retrying with authentication",
                status_code, challenge.realm
            );

            // Send ACK for the 401/407 response (required by RFC 3261)
            let via_branch =
                extract_via_branch(&response).unwrap_or_else(|| "z9hG4bKunknown".to_string());
            let to_tag = extract_to_tag(&response);
            let ack = build_ack(
                &target_uri,
                &from_uri,
                "PhoneCheck",
                &target_uri,
                to_tag.as_deref(),
                &call_id,
                &from_tag,
                cseq,
                local_addr,
                &via_branch,
            );
            transport.send(&ack).await?;

            // Compute digest response
            let digest_response = DigestResponse::compute(
                &challenge,
                &self.config.sip_username,
                &self.config.sip_password,
                "INVITE",
                &target_uri,
            );
            let authorization = digest_response.to_header();

            // Rebuild INVITE with Authorization header and incremented CSeq
            cseq += 1;
            let auth_invite = build_invite_with_auth(
                &target_uri,
                &from_uri,
                "PhoneCheck",
                &call_id,
                &from_tag,
                cseq,
                local_addr,
                rtp_port,
                external_rtp_addr,
                &authorization,
            );

            // Send authenticated INVITE
            response = match transport.send_invite_await_final(&auth_invite).await {
                Ok(r) => r,
                Err(e) => {
                    return Ok(CallResult {
                        connected: false,
                        audio_received: false,
                        audio_samples: Vec::new(),
                        error: Some(format!("No response after authentication: {}", e)),
                        sip_status: None,
                    });
                }
            };

            status_code = parse_status_code(&response).unwrap_or(0);
            debug!("Received response after auth: {}", status_code);
        }

        if status_code != 200 {
            let category = SipErrorCategory::from_status(status_code);
            let error_msg = format!(
                "Call rejected with status {}: {}",
                status_code,
                category.description()
            );

            // Log appropriate level based on error type
            if category.is_transient() {
                warn!("{}", error_msg);
            } else {
                // Permanent errors (auth, config) should be more visible
                tracing::error!("{}", error_msg);
            }

            return Ok(CallResult {
                connected: false,
                audio_received: false,
                audio_samples: Vec::new(),
                error: Some(error_msg),
                sip_status: Some(status_code),
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
            cseq,
            local_addr,
            &via_branch,
        );
        transport.send(&ack).await?;
        info!("Call connected, listening for audio...");

        // Receive RTP audio for specified duration (with cancellation support)
        let completed_normally = rtp_receiver
            .receive_for_cancellable(listen_duration, cancel_token.clone())
            .await?;

        let audio_samples = rtp_receiver.get_samples_f32();

        // Calculate audio duration in ms (16kHz sample rate after resampling)
        let audio_duration_ms = (audio_samples.len() as u64 * 1000) / 16000;

        // Check if we have enough audio (not just noise/glitches)
        let audio_received = audio_duration_ms >= self.config.min_audio_duration_ms;

        if !completed_normally {
            info!(
                "Call cancelled during audio receive - collected {} samples ({:.1}s)",
                audio_samples.len(),
                audio_samples.len() as f32 / 16000.0
            );
        } else if !audio_samples.is_empty() && !audio_received {
            warn!(
                "Received {} audio samples ({:.1}s) but below minimum threshold ({}ms)",
                audio_samples.len(),
                audio_samples.len() as f32 / 16000.0,
                self.config.min_audio_duration_ms
            );
        } else {
            info!(
                "Received {} audio samples ({:.1}s)",
                audio_samples.len(),
                audio_samples.len() as f32 / 16000.0
            );
        }

        // Always send BYE to end call cleanly (even on cancellation)
        let bye = build_bye(
            &target_uri,
            &from_uri,
            "PhoneCheck",
            &target_uri,
            to_tag.as_deref(),
            &call_id,
            &from_tag,
            cseq + 1, // BYE uses next CSeq after last INVITE
            local_addr,
        );

        if let Err(e) = transport.send(&bye).await {
            warn!("Failed to send BYE: {}", e);
        } else {
            // Wait for BYE response (best effort, shorter timeout on cancellation)
            let bye_timeout = if completed_normally {
                Duration::from_secs(5)
            } else {
                Duration::from_secs(2) // Shorter timeout during shutdown
            };

            match transport.receive(bye_timeout).await {
                Ok(bye_response) => {
                    let bye_status = parse_status_code(&bye_response).unwrap_or(0);
                    debug!("BYE response: {}", bye_status);
                }
                Err(e) => {
                    if completed_normally {
                        warn!("No response to BYE: {}", e);
                    } else {
                        debug!("No response to BYE during shutdown: {}", e);
                    }
                }
            }
        }

        info!(
            "Call ended{}",
            if completed_normally { "" } else { " (shutdown)" }
        );

        // If cancelled, return partial result but mark as connected
        // The caller can decide whether to use the partial audio
        Ok(CallResult {
            connected: true,
            audio_received: audio_received && completed_normally,
            audio_samples,
            error: if completed_normally {
                None
            } else {
                Some("Call cancelled by shutdown".to_string())
            },
            sip_status: Some(200),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sip_error_category_auth() {
        assert_eq!(SipErrorCategory::from_status(401), SipErrorCategory::AuthRequired);
        assert_eq!(SipErrorCategory::from_status(407), SipErrorCategory::AuthRequired);
    }

    #[test]
    fn test_sip_error_category_not_found() {
        assert_eq!(SipErrorCategory::from_status(404), SipErrorCategory::NotFound);
    }

    #[test]
    fn test_sip_error_category_busy() {
        assert_eq!(SipErrorCategory::from_status(486), SipErrorCategory::Busy);
        assert_eq!(SipErrorCategory::from_status(600), SipErrorCategory::Busy);
        assert_eq!(SipErrorCategory::from_status(603), SipErrorCategory::Busy);
    }

    #[test]
    fn test_sip_error_category_timeout() {
        assert_eq!(SipErrorCategory::from_status(408), SipErrorCategory::Timeout);
        assert_eq!(SipErrorCategory::from_status(480), SipErrorCategory::Timeout);
        assert_eq!(SipErrorCategory::from_status(504), SipErrorCategory::Timeout);
    }

    #[test]
    fn test_sip_error_category_server() {
        assert_eq!(SipErrorCategory::from_status(500), SipErrorCategory::ServerError);
        assert_eq!(SipErrorCategory::from_status(503), SipErrorCategory::ServerError);
        assert_eq!(SipErrorCategory::from_status(599), SipErrorCategory::ServerError);
    }

    #[test]
    fn test_sip_error_category_client() {
        assert_eq!(SipErrorCategory::from_status(400), SipErrorCategory::ClientError);
        assert_eq!(SipErrorCategory::from_status(403), SipErrorCategory::ClientError);
        assert_eq!(SipErrorCategory::from_status(415), SipErrorCategory::ClientError);
    }

    #[test]
    fn test_sip_error_category_redirect() {
        assert_eq!(SipErrorCategory::from_status(301), SipErrorCategory::Redirect);
        assert_eq!(SipErrorCategory::from_status(302), SipErrorCategory::Redirect);
    }

    #[test]
    fn test_sip_error_category_unknown() {
        assert_eq!(SipErrorCategory::from_status(100), SipErrorCategory::Unknown);
        assert_eq!(SipErrorCategory::from_status(200), SipErrorCategory::Unknown);
        assert_eq!(SipErrorCategory::from_status(0), SipErrorCategory::Unknown);
    }

    #[test]
    fn test_sip_error_is_transient() {
        // Transient errors
        assert!(SipErrorCategory::Busy.is_transient());
        assert!(SipErrorCategory::Timeout.is_transient());
        assert!(SipErrorCategory::ServerError.is_transient());

        // Non-transient errors
        assert!(!SipErrorCategory::AuthRequired.is_transient());
        assert!(!SipErrorCategory::NotFound.is_transient());
        assert!(!SipErrorCategory::ClientError.is_transient());
        assert!(!SipErrorCategory::Redirect.is_transient());
        assert!(!SipErrorCategory::Unknown.is_transient());
    }

    #[test]
    fn test_sip_error_descriptions() {
        // All categories should have non-empty descriptions
        assert!(!SipErrorCategory::AuthRequired.description().is_empty());
        assert!(!SipErrorCategory::NotFound.description().is_empty());
        assert!(!SipErrorCategory::Busy.description().is_empty());
        assert!(!SipErrorCategory::Timeout.description().is_empty());
        assert!(!SipErrorCategory::ServerError.description().is_empty());
        assert!(!SipErrorCategory::ClientError.description().is_empty());
        assert!(!SipErrorCategory::Redirect.description().is_empty());
        assert!(!SipErrorCategory::Unknown.description().is_empty());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// from_status never panics for any u16
        #[test]
        fn from_status_never_panics(code: u16) {
            let _ = SipErrorCategory::from_status(code);
        }

        /// All SIP status codes in valid range get classified
        #[test]
        fn all_sip_codes_classified(code in 100u16..700u16) {
            let category = SipErrorCategory::from_status(code);
            // Should return a valid category (not panic)
            let _ = category.description();
            let _ = category.is_transient();
        }
    }
}
