# voip.ms API Research

## SMS API (sendSMS)

### Endpoint
```
https://voip.ms/api/v1/rest.php
```

### Required Parameters
| Parameter | Description | Example |
|-----------|-------------|---------|
| `api_username` | Main account email | `user@example.com` |
| `api_password` | API password (set in portal) | `yourpassword` |
| `method` | API method name | `sendSMS` |
| `did` | Source DID (SMS-enabled) | `5551234567` |
| `dst` | Destination number | `5559876543` |
| `message` | SMS content (max 160 chars) | `Hello World` |

### Phone Number Format
- **NANPA format**: 10-digit number without country code (e.g., `5551234567`)
- **E.164 format**: With `+` and country code (e.g., `+15551234567`)
- voip.ms supports both; NANPA is simpler for North America

### Message Length
- Standard SMS: 160 characters max
- Longer messages may be split into multiple segments (test needed)

### Response Format
```json
{"status": "success"}
```
or
```json
{"status": "error", "message": "Error description"}
```

### Rate Limits
- Not officially documented
- Community reports suggest reasonable limits for normal usage

## SIP Authentication

### Two Methods Available

#### 1. IP Authentication (Recommended for servers)
- Configure in voip.ms portal: Sub Accounts → Create/Edit → Authentication Type: "Static IP"
- Enter server's public IP address
- No password required in SIP messages
- Account won't show "registered" (this is normal)
- **Requires static public IP**

#### 2. SIP Digest Authentication
- Standard RFC 2617/3261 digest auth
- Server responds with 401/407 and WWW-Authenticate header
- Client must compute MD5 response

### Current Implementation
The code uses IP authentication (password loaded but unused). This is the simpler approach if the server has a static IP.

## Existing Client Libraries

### Python
- [python-voipms](https://github.com/4doom4/python-voipms) - Full API wrapper
- [voipms-python](https://github.com/dtesfai/voipms-python) - Alternative wrapper

### Node.js
- [VoIP.ms-Node.js-Example](https://github.com/ikifar2012/VoIP.ms-Node.js-Example) - SMS example

## Sources
- [voip.ms API Page](https://voip.ms/resources/api)
- [voip.ms Blog - IP Authentication](https://blog.voip.ms/blog/what-is-ip-authentication-and-how-it-works/)
- [VoIP-info Forum](https://www.voip-info.org/forum/threads/ip-based-authentication-voip-ms.15570/)
