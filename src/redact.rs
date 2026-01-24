/// PII redaction utilities for logging
///
/// Masks sensitive information like phone numbers to prevent
/// leaking PII in logs while still providing useful debugging info.

/// Redact a phone number, keeping only the last 4 digits visible.
/// Example: "5551234567" -> "******4567"
pub fn phone_number(phone: &str) -> String {
    let digits: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();

    if digits.len() <= 4 {
        // Too short to meaningfully redact
        return "*".repeat(digits.len());
    }

    let visible = &digits[digits.len() - 4..];
    format!("{}{}",  "*".repeat(digits.len() - 4), visible)
}

/// Redact an email address, keeping domain visible.
/// Example: "user@example.com" -> "u***@example.com"
pub fn email(email: &str) -> String {
    if let Some(at_pos) = email.find('@') {
        if at_pos == 0 {
            return email.to_string();
        }
        let local = &email[..at_pos];
        let domain = &email[at_pos..];

        // Use chars to properly handle unicode
        let mut chars = local.chars();
        if let Some(first_char) = chars.next() {
            if chars.next().is_none() {
                // Single character local part
                format!("*{}", domain)
            } else {
                format!("{}***{}", first_char, domain)
            }
        } else {
            // Empty local part
            email.to_string()
        }
    } else {
        // Not a valid email, return as-is
        email.to_string()
    }
}

/// Redact a SIP URI, masking the user part.
/// Example: "sip:user@host.com" -> "sip:u***@host.com"
pub fn sip_uri(uri: &str) -> String {
    // Handle sip: or sips: prefix
    let (prefix, rest) = if uri.starts_with("sip:") {
        ("sip:", &uri[4..])
    } else if uri.starts_with("sips:") {
        ("sips:", &uri[5..])
    } else {
        return uri.to_string();
    };

    // Find the @ symbol
    if let Some(at_pos) = rest.find('@') {
        let user = &rest[..at_pos];
        let host = &rest[at_pos..];

        let redacted_user = if user.len() <= 1 {
            "*".to_string()
        } else {
            format!("{}***", &user[..1])
        };

        format!("{}{}{}", prefix, redacted_user, host)
    } else {
        // No @ symbol, might be just a host
        uri.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === phone_number tests ===

    #[test]
    fn test_phone_number_10_digit() {
        assert_eq!(phone_number("5551234567"), "******4567");
    }

    #[test]
    fn test_phone_number_with_formatting() {
        assert_eq!(phone_number("(555) 123-4567"), "******4567");
        assert_eq!(phone_number("555-123-4567"), "******4567");
        assert_eq!(phone_number("+1 555 123 4567"), "*******4567");
    }

    #[test]
    fn test_phone_number_short() {
        assert_eq!(phone_number("1234"), "****");
        assert_eq!(phone_number("123"), "***");
    }

    #[test]
    fn test_phone_number_e164() {
        assert_eq!(phone_number("+15551234567"), "*******4567");
    }

    // === email tests ===

    #[test]
    fn test_email_basic() {
        assert_eq!(email("user@example.com"), "u***@example.com");
    }

    #[test]
    fn test_email_single_char_local() {
        assert_eq!(email("a@example.com"), "*@example.com");
    }

    #[test]
    fn test_email_no_at() {
        assert_eq!(email("notanemail"), "notanemail");
    }

    #[test]
    fn test_email_empty_local() {
        assert_eq!(email("@example.com"), "@example.com");
    }

    // === sip_uri tests ===

    #[test]
    fn test_sip_uri_basic() {
        assert_eq!(sip_uri("sip:user@host.com"), "sip:u***@host.com");
    }

    #[test]
    fn test_sip_uri_phone_number() {
        assert_eq!(sip_uri("sip:5551234567@voip.ms"), "sip:5***@voip.ms");
    }

    #[test]
    fn test_sip_uri_sips() {
        assert_eq!(sip_uri("sips:user@host.com"), "sips:u***@host.com");
    }

    #[test]
    fn test_sip_uri_no_user() {
        assert_eq!(sip_uri("sip:host.com"), "sip:host.com");
    }

    #[test]
    fn test_sip_uri_not_sip() {
        assert_eq!(sip_uri("http://example.com"), "http://example.com");
    }

    #[test]
    fn test_sip_uri_single_char_user() {
        assert_eq!(sip_uri("sip:u@host.com"), "sip:*@host.com");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// phone_number redaction never panics
        #[test]
        fn phone_redaction_never_panics(s in ".*") {
            let _ = phone_number(&s);
        }

        /// email redaction never panics
        #[test]
        fn email_redaction_never_panics(s in ".*") {
            let _ = email(&s);
        }

        /// sip_uri redaction never panics
        #[test]
        fn sip_uri_redaction_never_panics(s in ".*") {
            let _ = sip_uri(&s);
        }

        /// phone_number always shows exactly 4 trailing digits for long numbers
        #[test]
        fn phone_keeps_last_4(digits in "[0-9]{5,15}") {
            let redacted = phone_number(&digits);
            assert!(redacted.ends_with(&digits[digits.len()-4..]));
        }

        /// redacted phone length matches original digit count
        #[test]
        fn phone_length_preserved(digits in "[0-9]{5,15}") {
            let redacted = phone_number(&digits);
            // All characters in redacted should be either * or digit
            let redacted_len: usize = redacted.chars()
                .filter(|c| *c == '*' || c.is_ascii_digit())
                .count();
            assert_eq!(redacted_len, digits.len());
        }
    }
}
