/// STUN client for NAT traversal
///
/// Discovers public IP address by sending a STUN Binding Request
/// to a STUN server and parsing the XOR-MAPPED-ADDRESS response.
///
/// Reference: RFC 5389 - Session Traversal Utilities for NAT (STUN)

use anyhow::{Context, Result};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Duration;
use tracing::{debug, info, warn};

/// STUN message types
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_RESPONSE: u16 = 0x0101;

/// STUN attribute types
const MAPPED_ADDRESS: u16 = 0x0001;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// STUN magic cookie (RFC 5389)
const MAGIC_COOKIE: u32 = 0x2112A442;

/// Default STUN request timeout
/// 3 seconds is generous for a single UDP round-trip to public STUN servers.
/// Most responses arrive within 100-500ms; 3s handles slow networks/servers.
const STUN_TIMEOUT: Duration = Duration::from_secs(3);

/// Discover public IP address using STUN
///
/// Returns the public SocketAddr as seen by the STUN server,
/// or None if STUN discovery fails.
pub async fn discover_public_address(stun_server: &str) -> Result<SocketAddr> {
    // Resolve STUN server address
    let server_addr = stun_server
        .to_socket_addrs()
        .context(format!("Failed to resolve STUN server: {}", stun_server))?
        .next()
        .context("No addresses found for STUN server")?;

    info!("Querying STUN server {} for public address", stun_server);

    // Run blocking STUN query in a separate thread
    let result = tokio::task::spawn_blocking(move || {
        stun_binding_request(server_addr)
    })
    .await
    .context("STUN task failed")??;

    info!("STUN discovered public address: {}", result);
    Ok(result)
}

/// Perform synchronous STUN binding request using a provided socket
pub fn stun_binding_request_on_socket(socket: &UdpSocket, server_addr: SocketAddr) -> Result<SocketAddr> {
    socket
        .set_read_timeout(Some(STUN_TIMEOUT))
        .context("Failed to set socket timeout")?;

    // Generate transaction ID (96 bits = 12 bytes)
    let transaction_id: [u8; 12] = rand::random();

    // Build STUN Binding Request
    let request = build_binding_request(&transaction_id);

    // Send request
    socket
        .send_to(&request, server_addr)
        .context("Failed to send STUN request")?;

    debug!("Sent STUN Binding Request to {}", server_addr);

    // Receive response
    let mut buf = [0u8; 512];
    let (len, _) = socket
        .recv_from(&mut buf)
        .context("Failed to receive STUN response (timeout?)")?;

    // Parse response
    parse_binding_response(&buf[..len], &transaction_id)
}

/// Perform synchronous STUN binding request
fn stun_binding_request(server_addr: SocketAddr) -> Result<SocketAddr> {
    // Create UDP socket
    let socket = UdpSocket::bind("0.0.0.0:0")
        .context("Failed to bind STUN socket")?;
    stun_binding_request_on_socket(&socket, server_addr)
}

/// Build a STUN Binding Request message
fn build_binding_request(transaction_id: &[u8; 12]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(20);

    // Message Type: Binding Request (0x0001)
    msg.extend_from_slice(&BINDING_REQUEST.to_be_bytes());

    // Message Length: 0 (no attributes)
    msg.extend_from_slice(&0u16.to_be_bytes());

    // Magic Cookie
    msg.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());

    // Transaction ID (12 bytes)
    msg.extend_from_slice(transaction_id);

    msg
}

/// Parse STUN Binding Response and extract mapped address
fn parse_binding_response(data: &[u8], expected_txn_id: &[u8; 12]) -> Result<SocketAddr> {
    if data.len() < 20 {
        anyhow::bail!("STUN response too short: {} bytes", data.len());
    }

    // Parse header
    let msg_type = u16::from_be_bytes([data[0], data[1]]);
    let msg_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    let magic = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let txn_id = &data[8..20];

    // Validate response
    if msg_type != BINDING_RESPONSE {
        anyhow::bail!("Unexpected STUN message type: 0x{:04x}", msg_type);
    }

    if magic != MAGIC_COOKIE {
        anyhow::bail!("Invalid STUN magic cookie");
    }

    if txn_id != expected_txn_id {
        anyhow::bail!("STUN transaction ID mismatch");
    }

    if data.len() < 20 + msg_len {
        anyhow::bail!("STUN message truncated");
    }

    // Parse attributes
    let mut offset = 20;
    let end = 20 + msg_len;

    while offset + 4 <= end {
        let attr_type = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let attr_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        offset += 4;

        if offset + attr_len > end {
            break;
        }

        let attr_data = &data[offset..offset + attr_len];

        match attr_type {
            XOR_MAPPED_ADDRESS => {
                return parse_xor_mapped_address(attr_data, &data[4..8]);
            }
            MAPPED_ADDRESS => {
                // Fallback for older STUN servers
                return parse_mapped_address(attr_data);
            }
            _ => {
                debug!("Ignoring STUN attribute type 0x{:04x}", attr_type);
            }
        }

        // Attributes are padded to 4-byte boundaries
        offset += (attr_len + 3) & !3;
    }

    anyhow::bail!("No MAPPED-ADDRESS or XOR-MAPPED-ADDRESS in STUN response")
}

/// Parse XOR-MAPPED-ADDRESS attribute (RFC 5389)
fn parse_xor_mapped_address(data: &[u8], magic_bytes: &[u8]) -> Result<SocketAddr> {
    if data.len() < 8 {
        anyhow::bail!("XOR-MAPPED-ADDRESS too short");
    }

    let family = data[1];
    let xport = u16::from_be_bytes([data[2], data[3]]);
    let port = xport ^ (MAGIC_COOKIE >> 16) as u16;

    match family {
        0x01 => {
            // IPv4
            let xaddr = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
            let addr = xaddr ^ MAGIC_COOKIE;
            let ip = std::net::Ipv4Addr::from(addr);
            Ok(SocketAddr::new(ip.into(), port))
        }
        0x02 => {
            // IPv6
            if data.len() < 20 {
                anyhow::bail!("XOR-MAPPED-ADDRESS IPv6 too short");
            }
            let _addr_bytes = [0u8; 16];
            // XOR with magic cookie + transaction ID
            let mut xor_bytes = [0u8; 16];
            xor_bytes[0..4].copy_from_slice(magic_bytes);
            // Transaction ID would be in bytes 8-20 of original message
            // For simplicity, we'll just handle IPv4 for now
            anyhow::bail!("IPv6 STUN not yet supported")
        }
        _ => {
            anyhow::bail!("Unknown address family: {}", family);
        }
    }
}

/// Parse MAPPED-ADDRESS attribute (legacy, RFC 3489)
fn parse_mapped_address(data: &[u8]) -> Result<SocketAddr> {
    if data.len() < 8 {
        anyhow::bail!("MAPPED-ADDRESS too short");
    }

    let family = data[1];
    let port = u16::from_be_bytes([data[2], data[3]]);

    match family {
        0x01 => {
            // IPv4
            let ip = std::net::Ipv4Addr::new(data[4], data[5], data[6], data[7]);
            Ok(SocketAddr::new(ip.into(), port))
        }
        _ => {
            anyhow::bail!("Unsupported address family: {}", family);
        }
    }
}

/// Discover public address using an existing Tokio UDP socket
pub async fn discover_public_address_tokio(socket: &tokio::net::UdpSocket, stun_server: &str) -> Result<SocketAddr> {
    let server_addr = stun_server
        .to_socket_addrs()
        .context(format!("Failed to resolve STUN server: {}", stun_server))?
        .next()
        .context("No addresses found for STUN server")?;

    // Generate transaction ID
    let transaction_id: [u8; 12] = rand::random();
    let request = build_binding_request(&transaction_id);

    // Send request
    socket.send_to(&request, server_addr).await?;

    // Receive response with timeout
    let mut buf = [0u8; 512];
    let result = tokio::time::timeout(STUN_TIMEOUT, socket.recv_from(&mut buf)).await;

    match result {
        Ok(Ok((len, _))) => parse_binding_response(&buf[..len], &transaction_id),
        Ok(Err(e)) => Err(anyhow::anyhow!("STUN receive error: {}", e)),
        Err(_) => Err(anyhow::anyhow!("STUN timeout")),
    }
}
///
/// If STUN server is configured, attempts STUN discovery.
/// On failure, logs a warning and returns None (caller should use local IP).
pub async fn discover_public_address_optional(stun_server: Option<&str>) -> Option<SocketAddr> {
    let server = stun_server?;

    match discover_public_address(server).await {
        Ok(addr) => Some(addr),
        Err(e) => {
            warn!("STUN discovery failed, using local IP: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_binding_request() {
        let txn_id = [0u8; 12];
        let request = build_binding_request(&txn_id);

        assert_eq!(request.len(), 20);
        // Message type
        assert_eq!(request[0], 0x00);
        assert_eq!(request[1], 0x01);
        // Message length
        assert_eq!(request[2], 0x00);
        assert_eq!(request[3], 0x00);
        // Magic cookie
        assert_eq!(&request[4..8], &MAGIC_COOKIE.to_be_bytes());
    }

    #[test]
    fn test_parse_xor_mapped_address_ipv4() {
        // XOR-MAPPED-ADDRESS for 192.0.2.1:32853
        // Port: 32853 (0x8055) XOR 0x2112 (high 16 bits of magic) = 0xA147
        // Addr: 192.0.2.1 (0xC0000201) XOR 0x2112A442 = 0xE112A643
        let data = [
            0x00, 0x01, // Reserved + Family (IPv4)
            0xA1, 0x47, // XOR'd port
            0xe1, 0x12, 0xa6, 0x43, // XOR'd address
        ];
        let magic = MAGIC_COOKIE.to_be_bytes();

        let result = parse_xor_mapped_address(&data, &magic).unwrap();
        assert_eq!(result.port(), 32853);
        assert_eq!(result.ip().to_string(), "192.0.2.1");
    }

    #[test]
    fn test_parse_mapped_address_ipv4() {
        let data = [
            0x00, 0x01, // Reserved + Family (IPv4)
            0x80, 0x55, // Port 32853
            192, 0, 2, 1, // Address
        ];

        let result = parse_mapped_address(&data).unwrap();
        assert_eq!(result.port(), 32853);
        assert_eq!(result.ip().to_string(), "192.0.2.1");
    }

    #[test]
    fn test_parse_binding_response_valid() {
        let txn_id = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

        // Build a valid response with XOR-MAPPED-ADDRESS
        let mut response = Vec::new();
        // Header
        response.extend_from_slice(&BINDING_RESPONSE.to_be_bytes());
        response.extend_from_slice(&12u16.to_be_bytes()); // Length = 12 (attribute)
        response.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        response.extend_from_slice(&txn_id);
        // XOR-MAPPED-ADDRESS attribute
        response.extend_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
        response.extend_from_slice(&8u16.to_be_bytes()); // Attr length
        response.extend_from_slice(&[
            0x00, 0x01, // Family
            0xA1, 0x47, // XOR'd port (32853 XOR 0x2112)
            0xe1, 0x12, 0xa6, 0x43, // XOR'd address
        ]);

        let result = parse_binding_response(&response, &txn_id).unwrap();
        assert_eq!(result.port(), 32853);
    }

    #[test]
    fn test_parse_binding_response_wrong_txn_id() {
        let txn_id = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let wrong_id = [0u8; 12];

        let mut response = Vec::new();
        response.extend_from_slice(&BINDING_RESPONSE.to_be_bytes());
        response.extend_from_slice(&0u16.to_be_bytes());
        response.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        response.extend_from_slice(&wrong_id);

        let result = parse_binding_response(&response, &txn_id);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("mismatch"));
    }

    #[tokio::test]
    async fn test_discover_public_address_optional_none() {
        // When no STUN server is configured, should return None immediately
        let result = discover_public_address_optional(None).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_discover_public_address_optional_invalid_server() {
        // When STUN server is unreachable, should return None (fallback)
        // Use an invalid hostname that will fail DNS resolution
        let result = discover_public_address_optional(Some("invalid.nonexistent.domain.test:3478")).await;
        assert!(result.is_none(), "Should gracefully return None on STUN failure");
    }

    #[tokio::test]
    async fn test_discover_public_address_timeout() {
        // Use a non-routable IP to trigger timeout (will fail within 3s)
        let result = discover_public_address_optional(Some("192.0.2.1:3478")).await;
        assert!(result.is_none(), "Should timeout gracefully");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Transaction ID is preserved in request
        #[test]
        fn binding_request_preserves_txn_id(txn_id: [u8; 12]) {
            let request = build_binding_request(&txn_id);
            prop_assert_eq!(&request[8..20], &txn_id);
        }

        /// parse_xor_mapped_address never panics
        #[test]
        fn parse_xor_never_panics(data: Vec<u8>) {
            let magic = MAGIC_COOKIE.to_be_bytes();
            let _ = parse_xor_mapped_address(&data, &magic);
        }

        /// parse_mapped_address never panics
        #[test]
        fn parse_mapped_never_panics(data: Vec<u8>) {
            let _ = parse_mapped_address(&data);
        }

        /// parse_binding_response never panics
        #[test]
        fn parse_response_never_panics(data: Vec<u8>, txn_id: [u8; 12]) {
            let _ = parse_binding_response(&data, &txn_id);
        }
    }
}
