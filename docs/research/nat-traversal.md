# NAT Traversal Research

## The Problem

When behind NAT:
1. SIP INVITE contains private IP in Contact header and SDP
2. Remote server can't send RTP to private IP
3. Result: one-way audio or no audio

## Common Solutions

### 1. STUN (Session Traversal Utilities for NAT)
- Client queries STUN server to discover public IP/port
- Uses discovered address in SIP/SDP
- Works with most NAT types except Symmetric NAT

**STUN Servers:**
- Google: `stun.l.google.com:19302`
- voip.ms: Check their documentation
- Twilio: `global.stun.twilio.com:3478`

### 2. TURN (Traversal Using Relays around NAT)
- Relay server forwards all traffic
- Works with any NAT type
- Higher latency, requires TURN server

### 3. SIP ALG (Application Layer Gateway)
- Router inspects and rewrites SIP
- Often buggy, usually recommended to DISABLE

### 4. Provider-side Handling
- Some providers handle NAT automatically
- voip.ms may send RTP to source of INVITE
- **Test first before implementing STUN**

## voip.ms Specific

From community forums:
- voip.ms generally handles NAT well
- If one-way audio occurs:
  1. First, disable SIP ALG on router
  2. If still broken, configure STUN
- STUN server recommendation needed (check voip.ms docs)

## Recommended Approach

### Phase 1: Test Without STUN
1. Make test call from behind NAT
2. If audio works both ways → voip.ms handles it
3. If one-way audio → implement STUN

### Phase 2: Implement STUN (if needed)
```rust
// Pseudocode
let public_addr = stun_client.discover_address("stun.l.google.com:19302")?;
// Use public_addr in:
// - SIP Contact header
// - SDP c= line
// - SDP m= line origin
```

### STUN Crates for Rust
- `stun-rs`: Pure Rust STUN implementation
- `webrtc-rs/stun`: From WebRTC project
- `stun_codec`: Low-level STUN codec

## Detailed STUN Implementation

### How STUN Works

1. Client sends Binding Request to STUN server (UDP)
2. STUN server responds with XOR-mapped address (client's public IP:port)
3. Client uses this address in SIP Contact and SDP

```rust
use std::net::UdpSocket;

async fn discover_public_address(stun_server: &str) -> Result<SocketAddr> {
    // 1. Create UDP socket
    let socket = UdpSocket::bind("0.0.0.0:0")?;

    // 2. Build STUN Binding Request
    let request = build_stun_binding_request();

    // 3. Send to STUN server
    socket.send_to(&request, stun_server)?;

    // 4. Receive response
    let mut buf = [0u8; 512];
    let (len, _) = socket.recv_from(&mut buf)?;

    // 5. Parse XOR-MAPPED-ADDRESS from response
    parse_xor_mapped_address(&buf[..len])
}
```

### NAT Types and STUN Compatibility

| NAT Type | STUN Works? | Notes |
|----------|-------------|-------|
| Full Cone | Yes | Any external host can send to mapped port |
| Restricted Cone | Yes | Only hosts we've sent to can reply |
| Port Restricted | Yes | Only same IP:port we sent to can reply |
| Symmetric | No | Port changes per destination - need TURN |

### SDP Modification for NAT

Original (behind NAT):
```
c=IN IP4 192.168.1.100
m=audio 16384 RTP/AVP 0
```

After STUN discovery (public IP 203.0.113.50, mapped port 54321):
```
c=IN IP4 203.0.113.50
m=audio 54321 RTP/AVP 0
```

## Sources
- [VideoSDK STUN VoIP Guide](https://www.videosdk.live/developer-hub/voip/stun-voip)
- [3CX STUN VoIP Tutorial](https://www.3cx.com/blog/voip-howto/stun-voip-1/)
- [TelVoIP One-Way Audio Fix](http://telvoip.com/index.php?article=fix-one-way-voip-audio-sip-nat-and-stun)
- [MDN WebRTC Protocols](https://developer.mozilla.org/en-US/docs/Web/API/WebRTC_API/Protocols)
- [STUN - Wikipedia](https://en.wikipedia.org/wiki/STUN)
- [NAT, STUN, TURN, and ICE](https://www.thirdlane.com/blog/nat-stun-turn-and-ice)
