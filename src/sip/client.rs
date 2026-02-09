/// SIP Client - Makes outbound calls and manages call state
/// Implements basic SIP UAC (User Agent Client) functionality

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::lookup_host;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::digest::{extract_authenticate_header, DigestChallenge, DigestResponse};
use super::messages::{
    build_ack, build_bye, build_invite, build_invite_with_auth, build_register,
    build_register_with_auth, extract_rtp_address, extract_to_tag, extract_via_branch,
    generate_call_id, generate_tag, parse_status_code,
};
use super::transport::SipTransport;
use crate::config::Config;
use crate::rtp::RtpReceiver;

/// SIP client for making outbound calls
pub struct SipClient {
    config: std::sync::Arc<Config>,
    server_addr: SocketAddr,
    from_uri: String,
    target_uri: String,
    display_name: String,
}

/// Result of a phone check call
#[derive(Debug, Default)]
pub struct CallResult {
    pub connected: bool,
    pub audio_received: bool,
    pub audio_samples: Vec<f32>,
    pub error: Option<String>,
    pub sip_status: Option<u16>,
}

impl CallResult {
    pub fn success(audio_samples: Vec<f32>, audio_received: bool) -> Self {
        Self {
            connected: true,
            audio_received,
            audio_samples,
            error: None,
            sip_status: Some(200),
        }
    }

    pub fn failed(error: String) -> Self {
        Self { connected: false, error: Some(error), ..Default::default() }
    }

    pub fn failed_with_status(status: u16, error: String) -> Self {
        Self { connected: false, sip_status: Some(status), error: Some(error), ..Default::default() }
    }
}

/// Classify SIP error codes for better error handling and reporting
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SipErrorCategory {
    AuthRequired,
    NotFound,
    Busy,
    Timeout,
    ServerError,
    ClientError,
    Redirect,
    Unknown,
}

impl SipErrorCategory {
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

    pub fn description(&self) -> &'static str {
        match self {
            SipErrorCategory::AuthRequired => "SIP authentication required",
            SipErrorCategory::NotFound => "Number not found",
            SipErrorCategory::Busy => "Line busy or call declined",
            SipErrorCategory::Timeout => "Call timeout",
            SipErrorCategory::ServerError => "SIP server error",
            SipErrorCategory::ClientError => "SIP client error",
            SipErrorCategory::Redirect => "Call redirected",
            SipErrorCategory::Unknown => "Unknown SIP error",
        }
    }
}

impl SipClient {
    pub async fn new(config: std::sync::Arc<Config>) -> Result<Self> {
        let server_host = format!("{}:{}", config.sip_server, config.sip_port);
        let server_addr = lookup_host(&server_host)
            .await
            .context(format!("Failed to resolve SIP server: {}", server_host))?
            .next()
            .context("No addresses found for SIP server")?;

        info!("SIP server resolved to {}", server_addr);

        let from_uri = format!("sip:{}@{}", config.sip_username, config.sip_server);
        let target_uri = format!("sip:{}@{}", config.target_phone, config.sip_server);

        Ok(Self {
            config,
            server_addr,
            from_uri,
            target_uri,
            display_name: "PhoneCheck".to_string(),
        })
    }

    pub async fn make_test_call_cancellable(
        &self,
        listen_duration: Duration,
        cancel_token: CancellationToken,
    ) -> Result<CallResult> {
        // Register with SIP server to authorize our IP for outbound calls.
        // This is essential when public IP changes (DHCP, location change).
        // Non-fatal: if registration fails, we still attempt the call.
        if let Err(e) = self.register().await {
            warn!("SIP registration failed: {} - proceeding with call attempt", e);
        }
        let rtp_receiver = RtpReceiver::bind(0).await?;
        self.make_test_call_with_receiver(listen_duration, rtp_receiver, cancel_token).await
    }

    pub async fn make_test_call_with_receiver(
        &self,
        listen_duration: Duration,
        mut rtp_receiver: RtpReceiver,
        cancel_token: CancellationToken,
    ) -> Result<CallResult> {
        let rtp_port = rtp_receiver.local_port()?;
        let transport = SipTransport::new(self.server_addr).await?;
        let local_addr = transport.local_addr()?;
        let call_id = generate_call_id(&local_addr.ip().to_string());
        let from_tag = generate_tag();
        let mut cseq = 1u32;

        info!("Initiating call to {}", self.config.target_phone);

        let external_rtp_addr = if let Some(ref stun_server) = self.config.stun_server {
            match rtp_receiver.discover_public_address(stun_server).await {
                Ok(addr) => {
                    info!("STUN discovered public RTP address: {}", addr);
                    Some(addr)
                }
                Err(e) => {
                    warn!("STUN discovery for RTP failed: {}", e);
                    None
                }
            }
        } else {
            None
        };

        let invite = build_invite(&self.target_uri, &self.from_uri, &self.display_name, &call_id, &from_tag, cseq, local_addr, rtp_port, external_rtp_addr);

        let mut response = match transport.send_invite_await_final(&invite).await {
            Ok(r) => r,
            Err(e) => return Ok(CallResult::failed(format!("No response from server: {}", e))),
        };

        let mut status_code = parse_status_code(&response).unwrap_or(0);

        if status_code == 401 || status_code == 407 {
            let res = self.handle_auth(&transport, &response, &call_id, &from_tag, &mut cseq, local_addr, rtp_port, external_rtp_addr).await?;
            match res {
                Ok(r) => {
                    response = r;
                    status_code = parse_status_code(&response).unwrap_or(0);
                }
                Err(call_res) => return Ok(call_res),
            }
        }

        if status_code != 200 {
            let category = SipErrorCategory::from_status(status_code);
            return Ok(CallResult::failed_with_status(status_code, format!("{}: {}", status_code, category.description())));
        }

        let to_tag = extract_to_tag(&response);
        let via_branch = extract_via_branch(&response).unwrap_or_else(|| "z9hG4bKunknown".to_string());
        let ack = build_ack(&self.target_uri, &self.from_uri, &self.display_name, &self.target_uri, to_tag.as_deref(), &call_id, &from_tag, cseq, local_addr, &via_branch);
        transport.send(&ack).await?;

        if let Some(remote_rtp_addr) = extract_rtp_address(&response) {
            let _ = rtp_receiver.punch_nat(remote_rtp_addr).await;
        }

        info!("Call connected, listening for audio...");
        let completed_normally = rtp_receiver.receive_for_cancellable(listen_duration, cancel_token.clone()).await?;
        let audio_samples = rtp_receiver.get_samples_f32();
        let audio_received = crate::rtp::samples_to_duration_ms(audio_samples.len()) >= self.config.min_audio_duration_ms;

        self.terminate_call(&transport, &call_id, &from_tag, to_tag.as_deref(), cseq + 1, local_addr, completed_normally).await;

        if completed_normally {
            Ok(CallResult::success(audio_samples, audio_received))
        } else {
            let mut result = CallResult::success(audio_samples, audio_received);
            result.error = Some("Call cancelled".to_string());
            Ok(result)
        }
    }

    async fn handle_auth(
        &self,
        transport: &SipTransport,
        response: &str,
        call_id: &str,
        from_tag: &str,
        cseq: &mut u32,
        local_addr: SocketAddr,
        rtp_port: u16,
        external_rtp_addr: Option<SocketAddr>
    ) -> Result<std::result::Result<String, CallResult>> {
        let status_code = parse_status_code(response).unwrap_or(0);
        if self.config.sip_password.is_empty() {
            return Ok(Err(CallResult::failed_with_status(status_code, "No SIP_PASSWORD".to_string())));
        }

        let auth_header = match extract_authenticate_header(response) {
            Some(h) => h,
            None => return Ok(Err(CallResult::failed_with_status(status_code, "No WWW-Authenticate".to_string()))),
        };

        let challenge = match DigestChallenge::parse(&auth_header) {
            Some(c) => c,
            None => return Ok(Err(CallResult::failed_with_status(status_code, "Bad challenge".to_string()))),
        };

        let via_branch = extract_via_branch(response).unwrap_or_else(|| "z9hG4bKunknown".to_string());
        let to_tag = extract_to_tag(response);
        let ack = build_ack(&self.target_uri, &self.from_uri, &self.display_name, &self.target_uri, to_tag.as_deref(), call_id, from_tag, *cseq, local_addr, &via_branch);
        transport.send(&ack).await?;

        let digest = DigestResponse::compute(&challenge, &self.config.sip_username, &self.config.sip_password, "INVITE", &self.target_uri);
        *cseq += 1;
        let auth_invite = build_invite_with_auth(&self.target_uri, &self.from_uri, &self.display_name, call_id, from_tag, *cseq, local_addr, rtp_port, external_rtp_addr, &digest.to_header());

        match transport.send_invite_await_final(&auth_invite).await {
            Ok(r) => Ok(Ok(r)),
            Err(e) => Ok(Err(CallResult::failed(format!("No response after auth: {}", e)))),
        }
    }

    async fn terminate_call(
        &self,
        transport: &SipTransport,
        call_id: &str,
        from_tag: &str,
        to_tag: Option<&str>,
        cseq: u32,
        local_addr: SocketAddr,
        completed_normally: bool
    ) {
        let bye = build_bye(&self.target_uri, &self.from_uri, &self.display_name, &self.target_uri, to_tag, call_id, from_tag, cseq, local_addr);
        let _ = transport.send(&bye).await;
        let timeout = if completed_normally { Duration::from_secs(5) } else { Duration::from_secs(2) };
        let _ = transport.receive(timeout).await;
    }

    /// Register with the SIP server via SIP REGISTER + digest auth.
    /// Authorizes our current IP for outbound calls, which is essential
    /// when the public IP changes (DHCP renewal, location change, etc.).
    async fn register(&self) -> Result<()> {
        let transport = SipTransport::new(self.server_addr).await?;
        let local_addr = transport.local_addr()?;
        let call_id = generate_call_id(&local_addr.ip().to_string());
        let from_tag = generate_tag();
        let register_uri = format!("sip:{}", self.config.sip_server);

        info!("Registering with SIP server...");

        let register = build_register(
            &self.config.sip_server,
            &self.from_uri,
            &self.display_name,
            &call_id,
            &from_tag,
            1,
            local_addr,
        );

        let response = transport.send_invite_await_final(&register).await
            .context("REGISTER request timed out")?;
        let status = parse_status_code(&response).unwrap_or(0);

        if status == 200 {
            info!("SIP registration successful");
            return Ok(());
        }

        if status != 401 && status != 407 {
            anyhow::bail!("SIP registration rejected with status {}", status);
        }

        // Handle digest auth challenge
        if self.config.sip_password.is_empty() {
            anyhow::bail!("SIP server requires authentication but SIP_PASSWORD is empty");
        }

        let auth_header = extract_authenticate_header(&response)
            .context("No WWW-Authenticate header in 401/407 response")?;
        let challenge = DigestChallenge::parse(&auth_header)
            .context("Failed to parse digest challenge")?;

        let digest = DigestResponse::compute(
            &challenge,
            &self.config.sip_username,
            &self.config.sip_password,
            "REGISTER",
            &register_uri,
        );

        let auth_register = build_register_with_auth(
            &self.config.sip_server,
            &self.from_uri,
            &self.display_name,
            &call_id,
            &from_tag,
            2,
            local_addr,
            &digest.to_header(),
        );

        let response = transport.send_invite_await_final(&auth_register).await
            .context("Authenticated REGISTER timed out")?;
        let status = parse_status_code(&response).unwrap_or(0);

        if status == 200 {
            info!("SIP registration successful (authenticated)");
            Ok(())
        } else {
            anyhow::bail!("SIP registration failed with status {} after auth", status)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sip_error_category() {
        assert_eq!(SipErrorCategory::from_status(401), SipErrorCategory::AuthRequired);
        assert_eq!(SipErrorCategory::from_status(404), SipErrorCategory::NotFound);
        assert_eq!(SipErrorCategory::from_status(486), SipErrorCategory::Busy);
    }
}
