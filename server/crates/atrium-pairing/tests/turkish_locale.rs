//! Criterion 12, test plan 2.1 `uppercasing_is_ascii_only_under_turkish_locale`.
//!
//! In a Turkish locale, locale-sensitive uppercasing turns `i` into `İ`,
//! which would make a correct secret undecodable on the owner's own machine
//! (ADR-003 §3, step 2). The decoder uppercases with an ASCII-only mapping, so
//! the locale must make no difference. This test re-runs itself with
//! `LC_ALL=tr_TR.UTF-8` and compares what the child decoded with what this
//! process decodes.
//!
//! When the locale is not installed the test says so loudly and skips — except
//! under CI, which generates the locale first, where a missing locale fails
//! the test so it can never skip silently.

use std::process::Command;

use atrium_pairing::secret::PairingSecret;

const CHILD: &str = "ATRIUM_TURKISH_LOCALE_CHILD";
/// Lowercase, with `i`, `l` and `o` (read as 1, 1 and 0), hyphens and spaces.
const TYPED: &str = "i00g-40r4-0m30-e209-185g-r38e-1w";
const CANONICAL: &str = "100G40R40M30E209185GR38E1W";

fn decoded_hex(text: &str) -> String {
    let secret = PairingSecret::decode(text).expect("decodes");
    secret.expose().iter().map(|b| format!("{b:02x}")).collect()
}

fn turkish_locale_installed() -> bool {
    Command::new("locale")
        .arg("-a")
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout).lines().any(|l| {
                l.eq_ignore_ascii_case("tr_TR.utf8") || l.eq_ignore_ascii_case("tr_TR.UTF-8")
            })
        })
        .unwrap_or(false)
}

#[test]
fn uppercasing_is_ascii_only_under_turkish_locale() {
    if std::env::var_os(CHILD).is_some() {
        assert_eq!(std::env::var("LC_ALL").as_deref(), Ok("tr_TR.UTF-8"));
        println!("DECODED={}", decoded_hex(TYPED));
        return;
    }
    if !turkish_locale_installed() {
        assert!(
            std::env::var_os("CI").is_none(),
            "tr_TR.UTF-8 is not installed, and CI must generate it (locale-gen tr_TR.UTF-8)"
        );
        eprintln!(
            "\n*** SKIPPED uppercasing_is_ascii_only_under_turkish_locale: the tr_TR.UTF-8 \
             locale is not installed here (CI installs it). ***\n"
        );
        return;
    }
    let output = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "uppercasing_is_ascii_only_under_turkish_locale",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("LC_ALL", "tr_TR.UTF-8")
        .env("LANG", "tr_TR.UTF-8")
        .output()
        .expect("run the child");
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let child = stdout
        .lines()
        .find_map(|line| line.strip_prefix("DECODED="))
        .expect("the child decoded");
    assert_eq!(child, decoded_hex(CANONICAL));
    assert_eq!(child, decoded_hex(TYPED));
}
