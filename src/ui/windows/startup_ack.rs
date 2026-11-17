//! Optional launcher handshake: native core initialization, not physical output readiness.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_BYTES: usize = 16 * 1024;
static TERMINAL: AtomicBool = AtomicBool::new(false);

pub(super) fn ready() {
    publish(
        "core_ready",
        "Native core initialized; output readiness is separate",
    );
}

pub(super) fn failed(detail: &str) {
    publish("failed", detail);
}

fn publish(stage: &str, detail: &str) {
    let Some(path) = std::env::var_os("PRISMESTRA_STARTUP_ACK_PATH") else {
        return;
    };
    let Ok(nonce) = std::env::var("PRISMESTRA_STARTUP_ACK_NONCE") else {
        return;
    };
    let path = PathBuf::from(path);
    if !path.is_absolute() || !valid_nonce(&nonce) {
        return;
    }
    if TERMINAL.swap(true, Ordering::Relaxed) {
        return;
    }
    // Publication errors must never tear down an otherwise functioning Runtime.
    let _ = write(&path, &nonce, stage, detail);
}

fn valid_nonce(nonce: &str) -> bool {
    nonce.len() == 32 && nonce.bytes().all(|value| value.is_ascii_hexdigit())
}

fn write(path: &std::path::Path, nonce: &str, stage: &str, detail: &str) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "pid": std::process::id(),
        "nonce": nonce,
        "stage": stage,
        "detail": detail.chars().take(2048).collect::<String>(),
    }))?;
    crate::storage::atomic_write(path, &bytes, MAX_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_is_bounded_and_hexadecimal() {
        assert!(valid_nonce("0123456789abcdef0123456789ABCDEF"));
        for value in ["", "short", "0123456789abcdef0123456789abcdeg"] {
            assert!(!valid_nonce(value));
        }
    }

    #[test]
    fn atomic_ack_contains_identity_and_bounded_unicode_detail() {
        let dir = std::env::temp_dir().join(format!("prismestra-ack-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ack.json");
        write(
            &path,
            "0123456789abcdef0123456789abcdef",
            "failed",
            &"я".repeat(4000),
        )
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() <= MAX_BYTES);
        let ack: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(ack["version"], 1);
        assert_eq!(ack["pid"], std::process::id());
        assert_eq!(ack["stage"], "failed");
        assert_eq!(ack["detail"].as_str().unwrap().chars().count(), 2048);
        assert!(write(&dir.join("missing/ack.json"), "nonce", "core_ready", "").is_err());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
