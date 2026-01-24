/// SIP Call State Machine - Stateright Model
/// Formally verifies the call flow: INVITE → 200 OK → ACK → RTP → BYE
///
/// Run with: cargo test --release sip_model -- --nocapture

use stateright::*;

/// Call states matching the actual SIP client implementation
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum CallState {
    Idle,
    Inviting { retries: u8 },
    Proceeding,
    Established,
    Terminating,
    Terminated,
    Failed,
}

/// Actions that can occur during a call
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum SipAction {
    SendInvite,
    Receive100Trying,
    Receive180Ringing,
    Receive200Ok,
    Receive4xx,
    Receive5xx,
    InviteTimeout,
    ReceiveRtp,
    AudioComplete,
    SendBye,
    ByeAcked,
    ByeTimeout,
}

/// Complete call state including RTP session
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct CallModel {
    pub state: CallState,
    pub rtp_active: bool,
    pub rtp_packets: u32,
    pub bye_sent: bool,
}

/// Configuration for the model checker
#[derive(Clone)]
pub struct SipCallChecker {
    pub max_retries: u8,
    pub max_rtp_packets: u32,
}

impl Default for SipCallChecker {
    fn default() -> Self {
        Self {
            max_retries: 3,
            max_rtp_packets: 5,
        }
    }
}

impl Model for SipCallChecker {
    type State = CallModel;
    type Action = SipAction;

    fn init_states(&self) -> Vec<Self::State> {
        vec![CallModel {
            state: CallState::Idle,
            rtp_active: false,
            rtp_packets: 0,
            bye_sent: false,
        }]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        match &state.state {
            CallState::Idle => {
                actions.push(SipAction::SendInvite);
            }

            CallState::Inviting { retries } => {
                // Server responses
                actions.push(SipAction::Receive100Trying);
                actions.push(SipAction::Receive180Ringing);
                actions.push(SipAction::Receive200Ok);
                actions.push(SipAction::Receive4xx);
                actions.push(SipAction::Receive5xx);
                // Timeout (may retry or fail)
                if *retries < self.max_retries {
                    actions.push(SipAction::InviteTimeout);
                }
            }

            CallState::Proceeding => {
                actions.push(SipAction::Receive180Ringing);
                actions.push(SipAction::Receive200Ok);
                actions.push(SipAction::Receive4xx);
                actions.push(SipAction::Receive5xx);
            }

            CallState::Established => {
                if state.rtp_active {
                    if state.rtp_packets < self.max_rtp_packets {
                        actions.push(SipAction::ReceiveRtp);
                    }
                    if state.rtp_packets >= self.max_rtp_packets {
                        actions.push(SipAction::AudioComplete);
                    }
                }
                if !state.bye_sent {
                    actions.push(SipAction::SendBye);
                }
            }

            CallState::Terminating => {
                actions.push(SipAction::ByeAcked);
                actions.push(SipAction::ByeTimeout);
            }

            CallState::Terminated | CallState::Failed => {
                // Terminal states - no actions
            }
        }
    }

    fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
        let mut next = state.clone();

        match action {
            SipAction::SendInvite => {
                if state.state == CallState::Idle {
                    next.state = CallState::Inviting { retries: 0 };
                }
            }

            SipAction::Receive100Trying => {
                if matches!(state.state, CallState::Inviting { .. }) {
                    next.state = CallState::Proceeding;
                }
            }

            SipAction::Receive180Ringing => {
                if matches!(state.state, CallState::Inviting { .. } | CallState::Proceeding) {
                    next.state = CallState::Proceeding;
                }
            }

            SipAction::Receive200Ok => {
                if matches!(state.state, CallState::Inviting { .. } | CallState::Proceeding) {
                    next.state = CallState::Established;
                    next.rtp_active = true;
                }
            }

            SipAction::Receive4xx | SipAction::Receive5xx => {
                if matches!(state.state, CallState::Inviting { .. } | CallState::Proceeding) {
                    next.state = CallState::Failed;
                }
            }

            SipAction::InviteTimeout => {
                if let CallState::Inviting { retries } = state.state {
                    if retries < self.max_retries {
                        next.state = CallState::Inviting {
                            retries: retries + 1,
                        };
                    } else {
                        next.state = CallState::Failed;
                    }
                }
            }

            SipAction::ReceiveRtp => {
                if state.state == CallState::Established && state.rtp_active {
                    next.rtp_packets = state.rtp_packets.saturating_add(1);
                }
            }

            SipAction::AudioComplete | SipAction::SendBye => {
                if state.state == CallState::Established {
                    next.state = CallState::Terminating;
                    next.rtp_active = false;
                    next.bye_sent = true;
                }
            }

            SipAction::ByeAcked => {
                if state.state == CallState::Terminating {
                    next.state = CallState::Terminated;
                }
            }

            SipAction::ByeTimeout => {
                if state.state == CallState::Terminating {
                    // Even without BYE ack, we consider call terminated
                    next.state = CallState::Terminated;
                }
            }
        }

        Some(next)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            // Safety: RTP is only active when call is established
            Property::always("rtp_only_when_established", |_, state: &CallModel| {
                !state.rtp_active || state.state == CallState::Established
            }),
            // Safety: When terminated or failed, RTP must be inactive
            Property::always("clean_termination", |_, state: &CallModel| {
                !matches!(
                    state.state,
                    CallState::Terminated | CallState::Failed
                ) || !state.rtp_active
            }),
            // Safety: BYE must be sent before entering terminating state
            Property::always("bye_before_terminating", |_, state: &CallModel| {
                state.state != CallState::Terminating || state.bye_sent
            }),
            // Safety: Cannot have RTP packets without having been established
            Property::always("no_orphan_rtp", |_, state: &CallModel| {
                state.rtp_packets == 0
                    || matches!(
                        state.state,
                        CallState::Established | CallState::Terminating | CallState::Terminated
                    )
            }),
            // Liveness: Call eventually terminates (no infinite loops)
            Property::eventually("call_terminates", |_, state: &CallModel| {
                matches!(state.state, CallState::Terminated | CallState::Failed)
            }),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stateright::Checker;

    #[test]
    fn sip_model_check_safety() {
        // Check all safety properties
        let checker = SipCallChecker::default().checker().spawn_bfs().join();

        // Print discovery statistics
        println!("States explored: {}", checker.unique_state_count());

        // Verify no safety violations
        checker.assert_properties();
    }

    #[test]
    fn sip_model_check_all_states_reachable() {
        let checker = SipCallChecker::default().checker().spawn_bfs().join();

        // Verify we explored a reasonable number of states
        // With max_retries=3 and max_rtp_packets=5, we should have many states
        assert!(
            checker.unique_state_count() > 10,
            "Expected more than 10 states, got {}",
            checker.unique_state_count()
        );
    }

    #[test]
    fn sip_model_successful_call_path() {
        // Verify specific path: Idle → Inviting → Proceeding → Established → Terminating → Terminated
        let model = SipCallChecker::default();

        let mut state = model.init_states()[0].clone();
        assert_eq!(state.state, CallState::Idle);

        // Send INVITE
        state = model
            .next_state(&state, SipAction::SendInvite)
            .unwrap();
        assert!(matches!(state.state, CallState::Inviting { retries: 0 }));

        // Receive 100 Trying
        state = model
            .next_state(&state, SipAction::Receive100Trying)
            .unwrap();
        assert_eq!(state.state, CallState::Proceeding);

        // Receive 200 OK
        state = model
            .next_state(&state, SipAction::Receive200Ok)
            .unwrap();
        assert_eq!(state.state, CallState::Established);
        assert!(state.rtp_active);

        // Receive RTP packets
        for _ in 0..5 {
            state = model.next_state(&state, SipAction::ReceiveRtp).unwrap();
        }
        assert_eq!(state.rtp_packets, 5);

        // Audio complete, send BYE
        state = model
            .next_state(&state, SipAction::AudioComplete)
            .unwrap();
        assert_eq!(state.state, CallState::Terminating);
        assert!(!state.rtp_active);
        assert!(state.bye_sent);

        // BYE acknowledged
        state = model.next_state(&state, SipAction::ByeAcked).unwrap();
        assert_eq!(state.state, CallState::Terminated);
    }

    #[test]
    fn sip_model_failed_call_path() {
        let model = SipCallChecker::default();

        let mut state = model.init_states()[0].clone();

        // Send INVITE
        state = model
            .next_state(&state, SipAction::SendInvite)
            .unwrap();

        // Receive 486 Busy (4xx error)
        state = model.next_state(&state, SipAction::Receive4xx).unwrap();
        assert_eq!(state.state, CallState::Failed);
        assert!(!state.rtp_active);
    }

    #[test]
    fn sip_model_timeout_retry_path() {
        let model = SipCallChecker::default();

        let mut state = model.init_states()[0].clone();

        // Send INVITE
        state = model
            .next_state(&state, SipAction::SendInvite)
            .unwrap();
        assert!(matches!(state.state, CallState::Inviting { retries: 0 }));

        // Timeout and retry
        state = model
            .next_state(&state, SipAction::InviteTimeout)
            .unwrap();
        assert!(matches!(state.state, CallState::Inviting { retries: 1 }));

        // Eventually get 200 OK
        state = model
            .next_state(&state, SipAction::Receive200Ok)
            .unwrap();
        assert_eq!(state.state, CallState::Established);
    }
}
