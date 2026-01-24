/// SIP Digest Authentication (RFC 2617 / RFC 7616)
/// Implements HTTP Digest authentication as used in SIP 401/407 challenges
///
/// Uses the md5 crate for hash computation - no custom crypto implementation.

use digest::Digest;
use md5::Md5;
use std::collections::HashMap;
use tracing::debug;

/// Parsed digest challenge from WWW-Authenticate or Proxy-Authenticate header
#[derive(Debug, Clone)]
pub struct DigestChallenge {
    pub realm: String,
    pub nonce: String,
    pub algorithm: DigestAlgorithm,
    pub qop: Option<String>,
    pub opaque: Option<String>,
    pub stale: bool,
}

/// Supported digest algorithms
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DigestAlgorithm {
    Md5,
    Md5Sess,
}

impl Default for DigestAlgorithm {
    fn default() -> Self {
        Self::Md5
    }
}

impl DigestChallenge {
    /// Parse a digest challenge from an authenticate header value
    /// Example: Digest realm="asterisk", nonce="1234", algorithm=MD5
    pub fn parse(header_value: &str) -> Option<Self> {
        // Remove "Digest " prefix if present
        let params_str = header_value.strip_prefix("Digest ").unwrap_or(header_value);

        let params = parse_params(params_str);

        let realm = params.get("realm")?.clone();
        let nonce = params.get("nonce")?.clone();

        let algorithm = match params.get("algorithm").map(|s| s.to_uppercase()).as_deref() {
            Some("MD5") | None => DigestAlgorithm::Md5,
            Some("MD5-SESS") => DigestAlgorithm::Md5Sess,
            Some(other) => {
                debug!("Unsupported digest algorithm: {}", other);
                return None;
            }
        };

        let qop = params.get("qop").cloned();
        let opaque = params.get("opaque").cloned();
        let stale = params.get("stale").map(|s| s.eq_ignore_ascii_case("true")).unwrap_or(false);

        Some(DigestChallenge {
            realm,
            nonce,
            algorithm,
            qop,
            opaque,
            stale,
        })
    }
}

/// Digest response for Authorization header
#[derive(Debug)]
pub struct DigestResponse {
    pub username: String,
    pub realm: String,
    pub nonce: String,
    pub uri: String,
    pub response: String,
    pub algorithm: DigestAlgorithm,
    pub qop: Option<String>,
    pub cnonce: Option<String>,
    pub nc: Option<String>,
    pub opaque: Option<String>,
}

impl DigestResponse {
    /// Compute digest response for a challenge
    pub fn compute(
        challenge: &DigestChallenge,
        username: &str,
        password: &str,
        method: &str,
        uri: &str,
    ) -> Self {
        // Generate client nonce for qop=auth
        let cnonce = if challenge.qop.is_some() {
            Some(generate_cnonce())
        } else {
            None
        };

        // Nonce count (always "00000001" for first use)
        let nc = if challenge.qop.is_some() {
            Some("00000001".to_string())
        } else {
            None
        };

        // Compute response hash
        let response = compute_response(
            challenge,
            username,
            password,
            method,
            uri,
            cnonce.as_deref(),
            nc.as_deref(),
        );

        DigestResponse {
            username: username.to_string(),
            realm: challenge.realm.clone(),
            nonce: challenge.nonce.clone(),
            uri: uri.to_string(),
            response,
            algorithm: challenge.algorithm,
            qop: challenge.qop.clone(),
            cnonce,
            nc,
            opaque: challenge.opaque.clone(),
        }
    }

    /// Format as Authorization header value
    pub fn to_header(&self) -> String {
        let mut parts = vec![
            format!("username=\"{}\"", self.username),
            format!("realm=\"{}\"", self.realm),
            format!("nonce=\"{}\"", self.nonce),
            format!("uri=\"{}\"", self.uri),
            format!("response=\"{}\"", self.response),
        ];

        match self.algorithm {
            DigestAlgorithm::Md5 => parts.push("algorithm=MD5".to_string()),
            DigestAlgorithm::Md5Sess => parts.push("algorithm=MD5-sess".to_string()),
        }

        if let Some(ref qop) = self.qop {
            parts.push(format!("qop={}", qop));
        }

        if let Some(ref cnonce) = self.cnonce {
            parts.push(format!("cnonce=\"{}\"", cnonce));
        }

        if let Some(ref nc) = self.nc {
            parts.push(format!("nc={}", nc));
        }

        if let Some(ref opaque) = self.opaque {
            parts.push(format!("opaque=\"{}\"", opaque));
        }

        format!("Digest {}", parts.join(", "))
    }
}

/// Compute the digest response hash per RFC 2617
fn compute_response(
    challenge: &DigestChallenge,
    username: &str,
    password: &str,
    method: &str,
    uri: &str,
    cnonce: Option<&str>,
    nc: Option<&str>,
) -> String {
    // HA1 = MD5(username:realm:password)
    let ha1 = md5_hex(&format!("{}:{}:{}", username, challenge.realm, password));

    // For MD5-sess: HA1 = MD5(MD5(username:realm:password):nonce:cnonce)
    let ha1 = if challenge.algorithm == DigestAlgorithm::Md5Sess {
        let cnonce = cnonce.unwrap_or("");
        md5_hex(&format!("{}:{}:{}", ha1, challenge.nonce, cnonce))
    } else {
        ha1
    };

    // HA2 = MD5(method:uri)
    let ha2 = md5_hex(&format!("{}:{}", method, uri));

    // Response hash depends on qop
    let response = if let Some(ref qop) = challenge.qop {
        if qop.contains("auth") {
            // With qop: MD5(HA1:nonce:nc:cnonce:qop:HA2)
            let nc = nc.unwrap_or("00000001");
            let cnonce = cnonce.unwrap_or("");
            let qop_value = if qop.contains("auth-int") {
                "auth-int"
            } else {
                "auth"
            };
            md5_hex(&format!(
                "{}:{}:{}:{}:{}:{}",
                ha1, challenge.nonce, nc, cnonce, qop_value, ha2
            ))
        } else {
            // Unknown qop, fall back to simple
            md5_hex(&format!("{}:{}:{}", ha1, challenge.nonce, ha2))
        }
    } else {
        // Without qop: MD5(HA1:nonce:HA2)
        md5_hex(&format!("{}:{}:{}", ha1, challenge.nonce, ha2))
    };

    response
}

/// Compute MD5 hash and return as lowercase hex string
fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

/// Generate a random client nonce
fn generate_cnonce() -> String {
    use rand::Rng;
    let bytes: [u8; 8] = rand::thread_rng().gen();
    hex::encode(bytes)
}

/// Parse key=value or key="value" parameters from header
fn parse_params(s: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();

    // Simple parser for key=value or key="value" pairs
    let mut remaining = s.trim();

    while !remaining.is_empty() {
        // Skip whitespace and commas
        remaining = remaining.trim_start_matches(|c: char| c.is_whitespace() || c == ',');

        if remaining.is_empty() {
            break;
        }

        // Find key
        let eq_pos = match remaining.find('=') {
            Some(pos) => pos,
            None => break,
        };

        let key = remaining[..eq_pos].trim().to_lowercase();
        remaining = &remaining[eq_pos + 1..].trim_start();

        // Parse value (quoted or unquoted)
        let (value, rest) = if remaining.starts_with('"') {
            // Quoted value
            let remaining = &remaining[1..]; // Skip opening quote
            if let Some(end_quote) = remaining.find('"') {
                let value = &remaining[..end_quote];
                let rest = &remaining[end_quote + 1..];
                (value.to_string(), rest)
            } else {
                // Unterminated quote - take rest
                (remaining.to_string(), "")
            }
        } else {
            // Unquoted value - ends at comma or whitespace
            let end = remaining
                .find(|c: char| c == ',' || c.is_whitespace())
                .unwrap_or(remaining.len());
            let value = &remaining[..end];
            let rest = &remaining[end..];
            (value.to_string(), rest)
        };

        params.insert(key, value);
        remaining = rest;
    }

    params
}

/// Find and extract WWW-Authenticate or Proxy-Authenticate header from SIP response
pub fn extract_authenticate_header(response: &str) -> Option<String> {
    for line in response.lines() {
        let lower = line.to_lowercase();
        if lower.starts_with("www-authenticate:") || lower.starts_with("proxy-authenticate:") {
            // Extract value after the colon
            if let Some(colon_pos) = line.find(':') {
                return Some(line[colon_pos + 1..].trim().to_string());
            }
        }
    }
    None
}

/// Add hex encoding since we're using the digest crate
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{:02x}", b)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_challenge() {
        let header = r#"Digest realm="asterisk", nonce="1234567890abcdef""#;
        let challenge = DigestChallenge::parse(header).unwrap();

        assert_eq!(challenge.realm, "asterisk");
        assert_eq!(challenge.nonce, "1234567890abcdef");
        assert_eq!(challenge.algorithm, DigestAlgorithm::Md5);
        assert!(challenge.qop.is_none());
    }

    #[test]
    fn test_parse_challenge_with_qop() {
        let header = r#"Digest realm="sip.example.com", nonce="abc123", qop="auth", algorithm=MD5"#;
        let challenge = DigestChallenge::parse(header).unwrap();

        assert_eq!(challenge.realm, "sip.example.com");
        assert_eq!(challenge.nonce, "abc123");
        assert_eq!(challenge.qop, Some("auth".to_string()));
        assert_eq!(challenge.algorithm, DigestAlgorithm::Md5);
    }

    #[test]
    fn test_parse_challenge_with_opaque() {
        let header = r#"Digest realm="test", nonce="xyz", opaque="opaque123""#;
        let challenge = DigestChallenge::parse(header).unwrap();

        assert_eq!(challenge.opaque, Some("opaque123".to_string()));
    }

    #[test]
    fn test_parse_challenge_md5_sess() {
        let header = r#"Digest realm="test", nonce="xyz", algorithm=MD5-sess"#;
        let challenge = DigestChallenge::parse(header).unwrap();

        assert_eq!(challenge.algorithm, DigestAlgorithm::Md5Sess);
    }

    #[test]
    fn test_parse_missing_realm() {
        let header = r#"Digest nonce="1234""#;
        assert!(DigestChallenge::parse(header).is_none());
    }

    #[test]
    fn test_parse_missing_nonce() {
        let header = r#"Digest realm="test""#;
        assert!(DigestChallenge::parse(header).is_none());
    }

    #[test]
    fn test_compute_response_simple() {
        // RFC 2617 test vectors
        let challenge = DigestChallenge {
            realm: "testrealm@host.com".to_string(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".to_string(),
            algorithm: DigestAlgorithm::Md5,
            qop: None,
            opaque: None,
            stale: false,
        };

        let response = DigestResponse::compute(
            &challenge,
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
        );

        // Known correct response from RFC 2617
        assert_eq!(response.response, "670fd8c2df070c60b045671b8b24ff02");
    }

    #[test]
    fn test_compute_response_with_qop() {
        let challenge = DigestChallenge {
            realm: "testrealm@host.com".to_string(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".to_string(),
            algorithm: DigestAlgorithm::Md5,
            qop: Some("auth".to_string()),
            opaque: Some("5ccc069c403ebaf9f0171e9517f40e41".to_string()),
            stale: false,
        };

        let response = DigestResponse::compute(
            &challenge,
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
        );

        // Response should be computed (exact value depends on cnonce)
        assert!(!response.response.is_empty());
        assert!(response.cnonce.is_some());
        assert_eq!(response.nc, Some("00000001".to_string()));
    }

    #[test]
    fn test_to_header_simple() {
        let response = DigestResponse {
            username: "user".to_string(),
            realm: "realm".to_string(),
            nonce: "nonce".to_string(),
            uri: "sip:user@host".to_string(),
            response: "abc123".to_string(),
            algorithm: DigestAlgorithm::Md5,
            qop: None,
            cnonce: None,
            nc: None,
            opaque: None,
        };

        let header = response.to_header();
        assert!(header.starts_with("Digest "));
        assert!(header.contains("username=\"user\""));
        assert!(header.contains("realm=\"realm\""));
        assert!(header.contains("response=\"abc123\""));
    }

    #[test]
    fn test_to_header_with_qop() {
        let response = DigestResponse {
            username: "user".to_string(),
            realm: "realm".to_string(),
            nonce: "nonce".to_string(),
            uri: "sip:user@host".to_string(),
            response: "abc123".to_string(),
            algorithm: DigestAlgorithm::Md5,
            qop: Some("auth".to_string()),
            cnonce: Some("xyz789".to_string()),
            nc: Some("00000001".to_string()),
            opaque: Some("opaque".to_string()),
        };

        let header = response.to_header();
        assert!(header.contains("qop=auth"));
        assert!(header.contains("cnonce=\"xyz789\""));
        assert!(header.contains("nc=00000001"));
        assert!(header.contains("opaque=\"opaque\""));
    }

    #[test]
    fn test_extract_authenticate_header() {
        let response = "SIP/2.0 401 Unauthorized\r\n\
                        Via: SIP/2.0/UDP host\r\n\
                        WWW-Authenticate: Digest realm=\"test\", nonce=\"123\"\r\n\
                        Content-Length: 0\r\n";

        let header = extract_authenticate_header(response);
        assert!(header.is_some());
        assert!(header.unwrap().contains("Digest"));
    }

    #[test]
    fn test_extract_proxy_authenticate_header() {
        let response = "SIP/2.0 407 Proxy Authentication Required\r\n\
                        Proxy-Authenticate: Digest realm=\"proxy\", nonce=\"456\"\r\n";

        let header = extract_authenticate_header(response);
        assert!(header.is_some());
        assert!(header.unwrap().contains("Digest"));
    }

    #[test]
    fn test_md5_hex() {
        // Known MD5 hash
        assert_eq!(md5_hex("hello"), "5d41402abc4b2a76b9719d911017c592");
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn test_parse_params() {
        let params = parse_params(r#"realm="test", nonce="123", algorithm=MD5"#);

        assert_eq!(params.get("realm"), Some(&"test".to_string()));
        assert_eq!(params.get("nonce"), Some(&"123".to_string()));
        assert_eq!(params.get("algorithm"), Some(&"MD5".to_string()));
    }

    #[test]
    fn test_parse_params_with_spaces() {
        let params = parse_params(r#"realm = "test" , nonce = "123""#);

        assert_eq!(params.get("realm"), Some(&"test".to_string()));
        assert_eq!(params.get("nonce"), Some(&"123".to_string()));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// MD5 hex output is always 32 characters
        #[test]
        fn md5_always_32_chars(input in ".*") {
            let hash = md5_hex(&input);
            prop_assert_eq!(hash.len(), 32);
        }

        /// MD5 hex output is always lowercase hex
        #[test]
        fn md5_always_lowercase_hex(input in ".*") {
            let hash = md5_hex(&input);
            prop_assert!(hash.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
        }

        /// Parse params never panics
        #[test]
        fn parse_params_never_panics(input in ".*") {
            let _ = parse_params(&input);
        }

        /// DigestChallenge::parse never panics
        #[test]
        fn parse_challenge_never_panics(input in ".*") {
            let _ = DigestChallenge::parse(&input);
        }

        /// Computed response is always 32 hex chars
        #[test]
        fn response_always_valid(
            realm in "[a-z]{3,10}",
            nonce in "[a-z0-9]{8,20}",
            username in "[a-z]{3,10}",
            password in "[a-z0-9]{4,16}",
            method in "(INVITE|REGISTER|BYE)",
            uri in "sip:[a-z]+@[a-z]+\\.[a-z]{2,4}"
        ) {
            let challenge = DigestChallenge {
                realm,
                nonce,
                algorithm: DigestAlgorithm::Md5,
                qop: None,
                opaque: None,
                stale: false,
            };

            let response = DigestResponse::compute(
                &challenge,
                &username,
                &password,
                &method,
                &uri,
            );

            prop_assert_eq!(response.response.len(), 32);
            prop_assert!(response.response.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }
}
