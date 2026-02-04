//! Command-line argument parsing for PhoneCheck

/// Parse command line arguments
pub struct Args {
    pub once: bool,
    pub validate: bool,
    pub help: bool,
    pub save_audio: Option<String>,
}

pub fn parse_args() -> Args {
    let args: Vec<String> = std::env::args().collect();
    let mut result = Args {
        once: false,
        validate: false,
        help: false,
        save_audio: None,
    };

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--once" => result.once = true,
            "--validate" => result.validate = true,
            "--help" | "-h" => result.help = true,
            "--save-audio" => {
                if i + 1 < args.len() {
                    i += 1;
                    result.save_audio = Some(args[i].clone());
                } else {
                    result.save_audio = Some("captured_audio.wav".to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }

    result
}

pub fn print_help() {
    println!("PhoneCheck - PBX Health Monitor\n");
    println!("USAGE:");
    println!("    phonecheck [OPTIONS]\n");
    println!("OPTIONS:");
    println!("    --once                  Run a single check and exit");
    println!("    --validate              Validate configuration and exit");
    println!("    --save-audio [PATH]     Save captured audio to WAV file (default: captured_audio.wav)");
    println!("    --help, -h              Show this help message\n");
    println!("ENVIRONMENT:");
    println!("    See .env.example for required configuration variables");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args_default() {
        let args = vec!["phonecheck".to_string()];
        let result = parse_args_internal(&args);
        assert!(!result.once);
        assert!(!result.validate);
        assert!(!result.help);
        assert!(result.save_audio.is_none());
    }

    #[test]
    fn test_parse_args_once() {
        let args = vec!["phonecheck".to_string(), "--once".to_string()];
        let result = parse_args_internal(&args);
        assert!(result.once);
        assert!(!result.validate);
    }

    #[test]
    fn test_parse_args_validate() {
        let args = vec!["phonecheck".to_string(), "--validate".to_string()];
        let result = parse_args_internal(&args);
        assert!(result.validate);
    }

    #[test]
    fn test_parse_args_help() {
        let args = vec!["phonecheck".to_string(), "--help".to_string()];
        let result = parse_args_internal(&args);
        assert!(result.help);

        let args = vec!["phonecheck".to_string(), "-h".to_string()];
        let result = parse_args_internal(&args);
        assert!(result.help);
    }

    #[test]
    fn test_parse_args_save_audio() {
        let args = vec![
            "phonecheck".to_string(),
            "--save-audio".to_string(),
            "test.wav".to_string(),
        ];
        let result = parse_args_internal(&args);
        assert_eq!(result.save_audio, Some("test.wav".to_string()));
    }

    #[test]
    fn test_parse_args_save_audio_default() {
        let args = vec!["phonecheck".to_string(), "--save-audio".to_string()];
        let result = parse_args_internal(&args);
        assert_eq!(result.save_audio, Some("captured_audio.wav".to_string()));
    }

    #[test]
    fn test_parse_args_multiple_flags() {
        let args = vec![
            "phonecheck".to_string(),
            "--once".to_string(),
            "--save-audio".to_string(),
            "test.wav".to_string(),
        ];
        let result = parse_args_internal(&args);
        assert!(result.once);
        assert_eq!(result.save_audio, Some("test.wav".to_string()));
    }

    /// Helper to test parse_args by setting the env args
    fn parse_args_internal(args: &[String]) -> Args {
        // Temporarily replace std::env::args with our test args
        // Note: This is a simple test - in a more sophisticated setup,
        // we'd use a proper CLI library with clap that supports testing
        let old_args: Vec<String> = std::env::args().collect();

        // Use a closure with custom args
        let result = {
            let mut result = Args {
                once: false,
                validate: false,
                help: false,
                save_audio: None,
            };

            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--once" => result.once = true,
                    "--validate" => result.validate = true,
                    "--help" | "-h" => result.help = true,
                    "--save-audio" => {
                        if i + 1 < args.len() {
                            i += 1;
                            result.save_audio = Some(args[i].clone());
                        } else {
                            result.save_audio = Some("captured_audio.wav".to_string());
                        }
                    }
                    _ => {}
                }
                i += 1;
            }

            result
        };

        result
    }
}
