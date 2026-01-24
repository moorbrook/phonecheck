# SIP Dialog State Machine Research

## RFC 3261 Overview

[RFC 3261](https://www.rfc-editor.org/rfc/rfc3261.html) defines SIP transaction and dialog state machines.

## Key Concepts

### Transaction vs Dialog
- **Transaction**: Single request + all responses (short-lived)
- **Dialog**: Peer-to-peer SIP relationship (persists for call duration)

### UAC vs UAS
- **UAC (User Agent Client)**: Initiates transactions (sends requests)
- **UAS (User Agent Server)**: Responds to transactions

A single UA acts as both:
- UAC when sending INVITE
- UAS when receiving BYE

## INVITE Client Transaction (UAC)

```
         ┌─────────────────────────────────────────────┐
         │                                             │
         │  INVITE sent                                │
         │       │                                     │
         │       ▼                                     │
         │  ┌─────────┐                                │
         │  │ CALLING │                                │
         │  └────┬────┘                                │
         │       │                                     │
         │       │ 1xx received                        │
         │       ▼                                     │
         │  ┌───────────┐                              │
         │  │PROCEEDING │◄────────────┐                │
         │  └─────┬─────┘             │ 1xx            │
         │        │                   │                │
         │        │ 2xx/3xx-6xx       │                │
         │        ▼                   │                │
         │  ┌───────────┐             │                │
         │  │ COMPLETED │─────────────┘                │
         │  └─────┬─────┘                              │
         │        │                                    │
         │        │ Timer D fires (32s for UDP)        │
         │        ▼                                    │
         │  ┌───────────┐                              │
         │  │TERMINATED │                              │
         │  └───────────┘                              │
         └─────────────────────────────────────────────┘
```

### State Transitions

| State | Event | Action | Next State |
|-------|-------|--------|------------|
| - | TU sends INVITE | Send INVITE, start Timer A & B | CALLING |
| CALLING | Timer A fires | Retransmit INVITE | CALLING |
| CALLING | 1xx received | - | PROCEEDING |
| CALLING | 2xx received | TU handles (not transaction) | TERMINATED |
| CALLING | 3xx-6xx received | Send ACK | COMPLETED |
| PROCEEDING | 1xx received | Pass to TU | PROCEEDING |
| PROCEEDING | 2xx received | TU handles | TERMINATED |
| PROCEEDING | 3xx-6xx received | Send ACK | COMPLETED |
| COMPLETED | 3xx-6xx received | Retransmit ACK | COMPLETED |
| COMPLETED | Timer D fires | - | TERMINATED |

## INVITE Server Transaction (UAS)

```
                 INVITE received
                       │
                       ▼
                 ┌───────────┐
                 │PROCEEDING │◄──────────────┐
                 └─────┬─────┘               │ INVITE
                       │                     │ retransmit
                       │                     │
     Send 1xx ────────►│                     │
                       │                     │
     Send 2xx ────────►│                     │
                       ▼                     │
                 ┌───────────┐               │
           ┌────►│ COMPLETED │───────────────┘
           │     └─────┬─────┘
           │           │
    ACK    │           │ Timer H fires (64*T1)
    recv   │           ▼
           │     ┌───────────┐
           │     │CONFIRMED  │
           │     └─────┬─────┘
           │           │
           │           │ Timer I fires (T4)
           └───────────┴───────────────►TERMINATED
```

## Dialog State Machine

```
                    ┌─────────────┐
                    │    INIT     │
                    └──────┬──────┘
                           │
              Send/Receive INVITE
                           │
                           ▼
                    ┌─────────────┐
        ┌──────────►│   EARLY     │
        │           └──────┬──────┘
        │                  │
  1xx provisional    2xx final response
        │                  │
        │                  ▼
        │           ┌─────────────┐
        └───────────│ CONFIRMED   │
                    └──────┬──────┘
                           │
                       BYE sent/received
                           │
                           ▼
                    ┌─────────────┐
                    │ TERMINATED  │
                    └─────────────┘
```

## Implementation for PhoneCheck

### Minimal State Machine

```rust
enum CallState {
    Idle,
    InviteSent,      // CALLING state
    Ringing,         // PROCEEDING (1xx received)
    Connected,       // 2xx received, dialog established
    Terminating,     // BYE sent
    Terminated,      // Call ended
}

struct SipDialog {
    state: CallState,
    call_id: String,
    local_tag: String,
    remote_tag: Option<String>,
    local_cseq: u32,
    remote_cseq: Option<u32>,
}

impl SipDialog {
    fn handle_response(&mut self, response: &SipResponse) -> Result<Action> {
        match (&self.state, response.status_code) {
            (CallState::InviteSent, 100..=199) => {
                self.state = CallState::Ringing;
                Ok(Action::Continue)
            }
            (CallState::InviteSent | CallState::Ringing, 200) => {
                self.remote_tag = response.to_tag();
                self.state = CallState::Connected;
                Ok(Action::SendAck)
            }
            (CallState::InviteSent | CallState::Ringing, 300..=699) => {
                self.state = CallState::Terminated;
                Ok(Action::SendAck)
            }
            _ => Ok(Action::None)
        }
    }
}
```

### Required Timers

| Timer | Duration | Purpose |
|-------|----------|---------|
| Timer A | T1 (500ms) initially | INVITE retransmit (doubles each time) |
| Timer B | 64*T1 (32s) | INVITE transaction timeout |
| Timer D | 32s (UDP) | Wait for response retransmits |
| Timer F | 64*T1 (32s) | Non-INVITE transaction timeout |

## Sources
- [RFC 3261](https://www.rfc-editor.org/rfc/rfc3261.html)
- [Tech-Invite RFC 3261 Chapter 17](https://www.tech-invite.com/y30/tinv-ietf-rfc-3261-7.html)
- [RFC 6141 - Re-INVITE Handling](https://www.rfc-editor.org/rfc/rfc6141.html)
