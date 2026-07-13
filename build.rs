//! Build script: compiles the SpeechAnalyzer transcription helper
//! (native/speech_helper.swift -> native/speech_helper) so that
//! `cargo build` produces a fully working setup on macOS 26+.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=native/speech_helper.swift");
    println!("cargo:rerun-if-changed=scripts/build_speech_helper.sh");

    if !cfg!(target_os = "macos") {
        println!("cargo:warning=speech_helper is macOS-only; transcription will be unavailable");
        return;
    }

    let status = Command::new("sh")
        .arg("scripts/build_speech_helper.sh")
        .status()
        .expect("failed to run scripts/build_speech_helper.sh");

    if !status.success() {
        panic!(
            "scripts/build_speech_helper.sh failed (exit: {status}). \
             Building the speech helper requires macOS 26+ and the Xcode \
             command line tools (swiftc)."
        );
    }
}
