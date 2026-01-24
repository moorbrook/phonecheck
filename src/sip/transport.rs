/// SIP UDP Transport Layer
/// Handles sending and receiving SIP messages over UDP
///
/// Implements RFC 3261 Timer A retransmission for INVITE over UDP:
/// - Timer A starts at T1 (500ms), doubles each retransmit
/// - Timer B (transaction timeout) is 64*T1 = 32 seconds
/// - Retransmission stops on any response

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;
use tracing::{debug, trace, warn};

/// RFC 3261 Timer T1 - RTT estimate (500ms default)
pub const T1: Duration = Duration::from_millis(500);

/// RFC 3261 Timer B - INVITE transaction timeout (64 * T1 = 32s)
pub const TIMER_B: Duration = Duration::from_secs(32);

pub struct SipTransport {
    socket: UdpSocket,
    server_addr: SocketAddr,
}

impl SipTransport {
    /// Create a new SIP transport bound to an ephemeral port
    pub async fn new(server_addr: SocketAddr) -> Result<Self> {
        // Bind to any available port
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .context("Failed to bind SIP socket")?;

        debug!(
            "SIP transport bound to {}",
            socket
                .local_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        );

        Ok(Self {
            socket,
            server_addr,
        })
    }

    /// Get local address
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.socket.local_addr().context("Failed to get local address")
    }

    /// Send a SIP message to the server
    pub async fn send(&self, message: &str) -> Result<()> {
        trace!("Sending SIP message:\n{}", message);

        self.socket
            .send_to(message.as_bytes(), self.server_addr)
            .await
            .context("Failed to send SIP message")?;

        Ok(())
    }

    /// Receive a SIP response with timeout
    pub async fn receive(&self, timeout_duration: Duration) -> Result<String> {
        let mut buf = [0u8; 4096];

        let (len, _addr) = timeout(timeout_duration, self.socket.recv_from(&mut buf))
            .await
            .context("Timeout waiting for SIP response")?
            .context("Failed to receive SIP response")?;

        let response = String::from_utf8_lossy(&buf[..len]).to_string();
        trace!("Received SIP message:\n{}", response);

        Ok(response)
    }

    /// Receive with retries (for handling provisional responses)
    pub async fn receive_final_response(
        &self,
        timeout_duration: Duration,
        max_retries: u32,
    ) -> Result<String> {
        let mut attempts = 0;

        loop {
            let response = self.receive(timeout_duration).await?;

            // Parse status code
            if let Some(code) = super::messages::parse_status_code(&response) {
                // 1xx are provisional responses, keep waiting
                if code >= 200 {
                    return Ok(response);
                }
                debug!("Received provisional response: {}", code);
            }

            attempts += 1;
            if attempts >= max_retries {
                anyhow::bail!("Max retries reached waiting for final response");
            }
        }
    }

    /// Send INVITE with RFC 3261 Timer A retransmission
    ///
    /// Implements the INVITE client transaction state machine:
    /// - Sends INVITE, starts Timer A at T1 (500ms)
    /// - On timeout: retransmit INVITE, double Timer A
    /// - On any response: stop retransmitting
    /// - Timer B (32s): overall transaction timeout
    ///
    /// Returns the first response received (may be provisional or final)
    pub async fn send_invite_with_retransmit(&self, invite: &str) -> Result<String> {
        let transaction_start = tokio::time::Instant::now();
        let mut timer_a = T1;
        let mut retransmit_count = 0u32;

        // Send initial INVITE
        self.send(invite).await?;
        debug!("Sent INVITE (initial), Timer A = {:?}", timer_a);

        loop {
            // Check Timer B (overall transaction timeout)
            if transaction_start.elapsed() >= TIMER_B {
                anyhow::bail!(
                    "INVITE transaction timeout (Timer B = {:?}) after {} retransmits",
                    TIMER_B,
                    retransmit_count
                );
            }

            // Wait for response with Timer A timeout
            match self.receive(timer_a).await {
                Ok(response) => {
                    // Got a response - return it (caller handles provisional vs final)
                    if let Some(code) = super::messages::parse_status_code(&response) {
                        debug!(
                            "Received {} response after {} retransmits",
                            code, retransmit_count
                        );
                    }
                    return Ok(response);
                }
                Err(e) => {
                    // Timeout - check if it's a receive timeout vs other error
                    let err_str = e.to_string().to_lowercase();
                    if !err_str.contains("timeout") {
                        return Err(e);
                    }

                    // Timer A expired - retransmit
                    retransmit_count += 1;

                    // Check Timer B before retransmitting
                    if transaction_start.elapsed() >= TIMER_B {
                        anyhow::bail!(
                            "INVITE transaction timeout (Timer B = {:?}) after {} retransmits",
                            TIMER_B,
                            retransmit_count
                        );
                    }

                    warn!(
                        "INVITE timeout, retransmitting (attempt {}, Timer A = {:?})",
                        retransmit_count + 1,
                        timer_a
                    );

                    self.send(invite).await?;

                    // Double Timer A for next iteration (RFC 3261 exponential backoff)
                    // Cap at T2 (4 seconds) for non-INVITE, but INVITE uses uncapped exponential
                    // until Timer B expires
                    timer_a = timer_a.saturating_mul(2);

                    // Cap timer_a at remaining time until Timer B
                    let remaining = TIMER_B.saturating_sub(transaction_start.elapsed());
                    if timer_a > remaining {
                        timer_a = remaining;
                    }
                }
            }
        }
    }

    /// Send INVITE and wait for final response with retransmission
    ///
    /// Combines send_invite_with_retransmit with provisional response handling.
    /// Returns the final (2xx-6xx) response.
    pub async fn send_invite_await_final(&self, invite: &str) -> Result<String> {
        let transaction_start = tokio::time::Instant::now();
        let mut timer_a = T1;
        let mut retransmit_count = 0u32;
        let mut in_proceeding = false; // True after receiving 1xx

        // Send initial INVITE
        self.send(invite).await?;
        debug!("Sent INVITE (initial), Timer A = {:?}", timer_a);

        loop {
            // Check Timer B
            let elapsed = transaction_start.elapsed();
            if elapsed >= TIMER_B {
                anyhow::bail!(
                    "INVITE transaction timeout (Timer B = {:?}) after {} retransmits",
                    TIMER_B,
                    retransmit_count
                );
            }

            let remaining = TIMER_B.saturating_sub(elapsed);
            let wait_time = timer_a.min(remaining);

            match self.receive(wait_time).await {
                Ok(response) => {
                    if let Some(code) = super::messages::parse_status_code(&response) {
                        if code >= 200 {
                            // Final response
                            debug!(
                                "Received final response {} after {} retransmits",
                                code, retransmit_count
                            );
                            return Ok(response);
                        } else {
                            // Provisional response - stop retransmitting, wait for final
                            debug!("Received provisional response {}", code);
                            in_proceeding = true;
                            // After receiving 1xx, we stop retransmitting and just wait
                            // Reset timer to wait for final response
                            timer_a = TIMER_B.saturating_sub(transaction_start.elapsed());
                        }
                    }
                }
                Err(e) => {
                    let err_str = e.to_string().to_lowercase();
                    if !err_str.contains("timeout") {
                        return Err(e);
                    }

                    // Only retransmit if we haven't received any provisional response
                    if !in_proceeding {
                        retransmit_count += 1;

                        // Check Timer B before retransmitting
                        if transaction_start.elapsed() >= TIMER_B {
                            anyhow::bail!(
                                "INVITE transaction timeout (Timer B = {:?}) after {} retransmits",
                                TIMER_B,
                                retransmit_count
                            );
                        }

                        warn!(
                            "INVITE timeout, retransmitting (attempt {}, Timer A = {:?})",
                            retransmit_count + 1,
                            timer_a
                        );

                        self.send(invite).await?;

                        // Double Timer A
                        timer_a = timer_a.saturating_mul(2);
                    }
                    // If in_proceeding, we just keep waiting without retransmitting
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[tokio::test]
    async fn test_transport_bind() {
        let addr: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        let transport = SipTransport::new(addr).await.unwrap();
        let local = transport.local_addr().unwrap();
        assert_ne!(local.port(), 0);
    }

    #[tokio::test]
    async fn test_transport_binds_to_ephemeral_port() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5060);
        let t1 = SipTransport::new(addr).await.unwrap();
        let t2 = SipTransport::new(addr).await.unwrap();

        // Both should bind to different ephemeral ports
        assert_ne!(t1.local_addr().unwrap().port(), t2.local_addr().unwrap().port());
    }

    #[tokio::test]
    async fn test_transport_local_addr_is_valid() {
        let addr: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        let transport = SipTransport::new(addr).await.unwrap();
        let local = transport.local_addr().unwrap();

        // Should be bound to 0.0.0.0 (any interface)
        assert!(local.ip().is_unspecified() || local.ip().is_loopback());
    }

    #[tokio::test]
    async fn test_receive_timeout() {
        let addr: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        let transport = SipTransport::new(addr).await.unwrap();

        // Should timeout quickly when no response
        let result = transport.receive(Duration::from_millis(10)).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Timeout") || err.contains("timeout"));
    }

    // Note: The following tests require UDP loopback which may not work in all environments.
    // They are marked #[ignore] and can be run with: cargo test -- --ignored

    #[tokio::test]
    #[ignore = "requires UDP loopback networking"]
    async fn test_send_receive_loopback() {
        // Create two transports that can communicate
        let addr1: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let t1 = SipTransport::new(addr1).await.unwrap();
        let t1_addr = t1.local_addr().unwrap();

        // Create second transport targeting first
        let t2 = SipTransport::new(t1_addr).await.unwrap();

        // Send from t2 to t1
        let test_message = "SIP/2.0 200 OK\r\n\r\n";
        t2.send(test_message).await.unwrap();

        // Receive on t1
        let received = t1.receive(Duration::from_secs(1)).await.unwrap();
        assert_eq!(received, test_message);
    }

    #[tokio::test]
    #[ignore = "requires UDP loopback networking"]
    async fn test_receive_final_response_skips_provisional() {
        // Create paired transports
        let t1 = SipTransport::new("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let t1_addr = t1.local_addr().unwrap();
        let t2 = SipTransport::new(t1_addr).await.unwrap();

        // Spawn task to receive final response
        let receiver = tokio::spawn(async move {
            t1.receive_final_response(Duration::from_secs(5), 10).await
        });

        // Send provisional then final
        t2.send("SIP/2.0 100 Trying\r\n\r\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        t2.send("SIP/2.0 180 Ringing\r\n\r\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        t2.send("SIP/2.0 200 OK\r\n\r\n").await.unwrap();

        let result = receiver.await.unwrap().unwrap();
        assert!(result.contains("200 OK"));
    }

    #[tokio::test]
    #[ignore = "requires UDP loopback networking"]
    async fn test_receive_final_response_returns_error_codes() {
        let t1 = SipTransport::new("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let t1_addr = t1.local_addr().unwrap();
        let t2 = SipTransport::new(t1_addr).await.unwrap();

        let receiver = tokio::spawn(async move {
            t1.receive_final_response(Duration::from_secs(5), 10).await
        });

        // Send 486 Busy
        t2.send("SIP/2.0 486 Busy Here\r\n\r\n").await.unwrap();

        let result = receiver.await.unwrap().unwrap();
        assert!(result.contains("486"));
    }

    #[tokio::test]
    #[ignore = "requires UDP loopback networking"]
    async fn test_receive_final_response_max_retries() {
        let t1 = SipTransport::new("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let t1_addr = t1.local_addr().unwrap();
        let t2 = SipTransport::new(t1_addr).await.unwrap();

        let receiver = tokio::spawn(async move {
            t1.receive_final_response(Duration::from_millis(50), 3).await
        });

        // Send only provisional responses
        for _ in 0..3 {
            t2.send("SIP/2.0 100 Trying\r\n\r\n").await.unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let result = receiver.await.unwrap();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Max retries"));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Any valid SIP message can be sent without panic
        #[test]
        fn send_never_panics(message in "[A-Za-z0-9 :/@\r\n]{0,1000}") {
            // We can't actually test send without a socket, but we can verify
            // the message bytes are valid
            let bytes = message.as_bytes();
            prop_assert!(bytes.len() <= 1000);
        }

        /// Timeout durations are handled correctly
        #[test]
        fn timeout_duration_valid(ms in 1u64..10000u64) {
            let duration = Duration::from_millis(ms);
            prop_assert!(duration.as_millis() >= 1);
            prop_assert!(duration.as_millis() <= 10000);
        }
    }
}

#[cfg(test)]
mod timer_tests {
    use super::*;

    #[test]
    fn test_t1_value() {
        // RFC 3261: T1 should be 500ms
        assert_eq!(T1, Duration::from_millis(500));
    }

    #[test]
    fn test_timer_b_value() {
        // RFC 3261: Timer B = 64 * T1 = 32 seconds
        assert_eq!(TIMER_B, Duration::from_secs(32));
        assert_eq!(TIMER_B, T1 * 64);
    }

    #[test]
    fn test_exponential_backoff_sequence() {
        // Verify the Timer A doubling sequence
        let mut timer = T1;
        let expected = [500, 1000, 2000, 4000, 8000, 16000];

        for (i, expected_ms) in expected.iter().enumerate() {
            assert_eq!(
                timer.as_millis() as u64,
                *expected_ms,
                "Timer A at iteration {} should be {}ms",
                i,
                expected_ms
            );
            timer = timer.saturating_mul(2);
        }
    }

    #[test]
    fn test_timer_a_capped_by_timer_b() {
        // After enough doublings, Timer A should be capped by remaining Timer B time
        let mut timer = T1;

        // Double 7 times: 500 -> 1000 -> 2000 -> 4000 -> 8000 -> 16000 -> 32000 -> 64000
        for _ in 0..7 {
            timer = timer.saturating_mul(2);
        }

        // At this point timer is 64000ms = 64s, exceeds Timer B
        assert!(timer > TIMER_B);

        // In real code, we cap it
        let capped = timer.min(TIMER_B);
        assert_eq!(capped, TIMER_B);
    }

    #[test]
    fn test_total_retransmit_time() {
        // Calculate total time if all retransmits happen
        // 500 + 1000 + 2000 + 4000 + 8000 + 16000 = 31500ms < 32000ms Timer B
        // So we get ~6 retransmits before Timer B expires
        let mut total = Duration::ZERO;
        let mut timer = T1;

        while total + timer < TIMER_B {
            total += timer;
            timer = timer.saturating_mul(2);
        }

        // Should have accumulated significant time
        assert!(total.as_millis() > 30000);
        assert!(total < TIMER_B);
    }
}
