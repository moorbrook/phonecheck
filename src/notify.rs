use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info, warn};

use crate::config::Config;

const MAX_RETRIES: u32 = 3;
const PUSHOVER_API_URL: &str = "https://api.pushover.net/1/messages.json";

#[derive(Debug, Deserialize)]
struct PushoverResponse {
    status: i32,
    #[serde(default)]
    errors: Option<Vec<String>>,
}

pub struct Notifier {
    client: reqwest::Client,
    user_key: String,
    api_token: String,
}

impl Notifier {
    pub fn new(config: &Config) -> Self {
        Self {
            client: reqwest::Client::new(),
            user_key: config.pushover_user_key.clone(),
            api_token: config.pushover_api_token.clone(),
        }
    }

    pub async fn send_alert(&self, message: &str) -> Result<()> {
        info!("Sending Pushover alert: {}", message);

        let mut last_error = None;

        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let backoff = Duration::from_secs(1 << attempt);
                warn!("Pushover attempt {} failed, retrying in {:?}...", attempt, backoff);
                sleep(backoff).await;
            }

            match self.try_send(message).await {
                Ok(()) => {
                    info!("Pushover alert sent successfully");
                    return Ok(());
                }
                Err(e) => {
                    last_error = Some(e);
                }
            }
        }

        let err = last_error.unwrap();
        error!("Failed to send Pushover alert after {} attempts: {}", MAX_RETRIES, err);
        Err(err)
    }

    async fn try_send(&self, message: &str) -> Result<()> {
        let params = [
            ("token", self.api_token.as_str()),
            ("user", self.user_key.as_str()),
            ("message", message),
            ("title", "PhoneCheck Alert"),
            ("priority", "1"), // High priority
        ];

        let response = self
            .client
            .post(PUSHOVER_API_URL)
            .form(&params)
            .send()
            .await
            .context("Failed to send Pushover request")?;

        let result: PushoverResponse = response
            .json()
            .await
            .context("Failed to parse Pushover response")?;

        if result.status == 1 {
            Ok(())
        } else {
            let err_msg = result
                .errors
                .map(|e| e.join(", "))
                .unwrap_or_else(|| "Unknown error".to_string());
            anyhow::bail!("Pushover API error: {}", err_msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pushover_response_success() {
        let json = r#"{"status": 1, "request": "abc123"}"#;
        let response: PushoverResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.status, 1);
        assert!(response.errors.is_none());
    }

    #[test]
    fn test_pushover_response_error() {
        let json = r#"{"status": 0, "errors": ["user key is invalid"]}"#;
        let response: PushoverResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.status, 0);
        assert_eq!(response.errors, Some(vec!["user key is invalid".to_string()]));
    }
}
