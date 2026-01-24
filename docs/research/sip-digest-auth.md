# SIP Digest Authentication Research

## IMPORTANT: Use Existing Libraries

**DO NOT roll your own crypto.** Use existing Rust crates:

### Recommended Crates

1. **[digest_auth](https://crates.io/crates/digest_auth)** - RFC 2069/2617/7616 compliant
   - Parse WWW-Authenticate headers
   - Compute responses automatically
   - Handles nonce counting

2. **[rvoip-sip-core](https://crates.io/crates/rvoip-sip-core)** - Full SIP stack with auth
   - SIP-specific authentication
   - Complete digest auth implementation

3. **[http-auth](https://crates.io/crates/http-auth)** - Parse and respond to challenges

### Usage with digest_auth
```rust
use digest_auth::{AuthContext, parse};

// Parse the 401 response header
let challenge = parse(www_authenticate_header)?;

// Create auth context
let context = AuthContext::new("username", "password", request_uri);

// Generate Authorization header
let authorization = challenge.respond(&context)?;
```

## RFC 2617/3261 Overview

Digest authentication prevents sending passwords in cleartext by using MD5 hashes.

## Authentication Flow

1. Client sends INVITE without credentials
2. Server responds with 401 Unauthorized + WWW-Authenticate header
3. Client computes response and resends INVITE with Authorization header
4. Server validates and proceeds

## Response Calculation Algorithm

From Sofia-SIP `auth_digest.c`:

```
A1 = MD5(username:realm:password)
A2 = MD5(method:uri)           # or MD5(method:uri:body_hash) for auth-int

response = MD5(A1:nonce:A2)                    # without qop
response = MD5(A1:nonce:nc:cnonce:qop:A2)      # with qop
```

### With Session (auth-sess)
```
A1sess = MD5(A1:nonce:cnonce)
```

## Key Parameters

| Parameter | Description | Example |
|-----------|-------------|---------|
| `realm` | Authentication domain | `voip.ms` |
| `nonce` | Server-provided unique value | Base64 string |
| `uri` | Request URI | `sip:5551234@voip.ms` |
| `qop` | Quality of protection | `auth` or `auth-int` |
| `nc` | Nonce count (hex, 8 chars) | `00000001` |
| `cnonce` | Client nonce | Random hex string |
| `algorithm` | Hash algorithm | `MD5` (default) |

## Implementation Approach

### Step 1: Parse WWW-Authenticate Header
```
WWW-Authenticate: Digest realm="voip.ms",nonce="abc123",qop="auth",algorithm=MD5
```

### Step 2: Generate Client Values
- `cnonce`: Random 16-byte hex string
- `nc`: "00000001" (increment per request)

### Step 3: Compute Response
```rust
fn compute_digest_response(
    username: &str,
    password: &str,
    realm: &str,
    nonce: &str,
    method: &str,
    uri: &str,
    cnonce: &str,
    nc: &str,
    qop: Option<&str>,
) -> String {
    let a1 = md5_hex(format!("{}:{}:{}", username, realm, password));
    let a2 = md5_hex(format!("{}:{}", method, uri));

    match qop {
        Some(q) => md5_hex(format!("{}:{}:{}:{}:{}:{}", a1, nonce, nc, cnonce, q, a2)),
        None => md5_hex(format!("{}:{}:{}", a1, nonce, a2)),
    }
}
```

### Step 4: Build Authorization Header
```
Authorization: Digest username="user",realm="voip.ms",nonce="abc123",
  uri="sip:5551234@voip.ms",response="computed_hash",algorithm=MD5,
  cnonce="client_nonce",nc=00000001,qop=auth
```

## voip.ms Specific

- Likely uses MD5 algorithm (standard)
- May or may not require qop
- Test with IP auth disabled to capture actual challenge

## Rust Implementation Notes

- Use `md5` crate for hashing
- Parse challenge with regex or simple string splitting
- Handle both 401 (WWW-Authenticate) and 407 (Proxy-Authenticate)

## Sources
- [Sofia-SIP auth_digest.c](https://github.com/freeswitch/sofia-sip/blob/master/libsofia-sip-ua/iptsec/auth_digest.c)
- [IETF Draft: Digest Auth Examples](https://datatracker.ietf.org/doc/html/draft-smith-sipping-auth-examples-01)
- [RFC 2617](https://www.rfc-editor.org/rfc/rfc2617)
