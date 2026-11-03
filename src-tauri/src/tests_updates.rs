//! Updating from inside the app, without a release server: when it is safe to restart, when a check is due, what the
//! notice makes of a release (major or not, the breaking changes apart), whether a build has a real key, where the
//! choice to check is kept, and how a refused signature is put into words. The Claude Code side of the restart (no
//! run can start once the restart is decided) is tested with the other runs, in `tests_claude_code.rs`.

use std::fs;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use tempfile::tempdir;

use crate::settings::{self, UpdateConfig};
use crate::updates::{
    Activity, Due, EVERY, FIRST_CHECK, Readiness, RunsChoice, SYNC_WAIT, install_failure, is_major, is_signature_error, key_problem, readiness,
    simulated_notes, simulated_version, split_notes, update_info, when_due,
};

fn idle() -> Activity {
    Activity::default()
}

// ---- whether it is safe to restart --------------------------------------------------------------------------

#[test]
fn with_nothing_going_on_it_installs_at_once() {
    for choice in [RunsChoice::Ask, RunsChoice::Wait, RunsChoice::Stop] {
        assert_eq!(readiness(&idle(), choice), Readiness::Ready { notes: vec![] });
    }
}

#[test]
fn a_running_sync_is_always_waited_for() {
    let github = Activity { github_syncing: true, ..idle() };
    let Readiness::Wait { waiting_for } = readiness(&github, RunsChoice::Ask) else { panic!("should wait") };
    assert_eq!(waiting_for, ["the GitHub sync to finish"]);

    let trello = Activity { trello_syncing: true, ..idle() };
    let Readiness::Wait { waiting_for } = readiness(&trello, RunsChoice::Stop) else { panic!("should wait") };
    assert_eq!(waiting_for, ["the Trello sync to finish"]);

    let both = Activity { github_syncing: true, trello_syncing: true, ..idle() };
    let Readiness::Wait { waiting_for } = readiness(&both, RunsChoice::Wait) else { panic!("should wait") };
    assert_eq!(waiting_for.len(), 2);
}

#[test]
fn changes_not_yet_pushed_dont_hold_it_up_but_are_mentioned() {
    // They are in the vault's files already, and the sync Ledger starts with pushes them.
    let pending = Activity { github_pending: true, ..idle() };
    let Readiness::Ready { notes } = readiness(&pending, RunsChoice::Ask) else { panic!("should be ready") };
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("aren't on GitHub yet"), "{notes:?}");
}

#[test]
fn claude_code_runs_are_the_persons_call() {
    let runs = Activity { runs: vec!["Call summary".into(), "Plan".into()], ..idle() };
    // Nobody has said yet: ask, naming the prompts, even while a sync also runs.
    assert_eq!(readiness(&runs, RunsChoice::Ask), Readiness::Ask { runs: vec!["Call summary".into(), "Plan".into()] });
    let syncing_too = Activity { github_syncing: true, ..runs.clone() };
    assert!(matches!(readiness(&syncing_too, RunsChoice::Ask), Readiness::Ask { .. }));

    // Wait for them, or for them to stop: either way it waits until they are over.
    let Readiness::Wait { waiting_for } = readiness(&runs, RunsChoice::Wait) else { panic!("should wait") };
    assert_eq!(waiting_for, ["Claude Code to finish “Call summary” and “Plan”"]);
    let Readiness::Wait { waiting_for } = readiness(&runs, RunsChoice::Stop) else { panic!("should wait") };
    assert_eq!(waiting_for, ["Claude Code to stop “Call summary” and “Plan”"]);

    let three = Activity { runs: vec!["A".into(), " ".into(), "C".into()], ..idle() };
    let Readiness::Wait { waiting_for } = readiness(&three, RunsChoice::Wait) else { panic!("should wait") };
    assert_eq!(waiting_for, ["Claude Code to finish “A”, “a prompt” and “C”"]);
}

#[test]
fn the_answer_reaches_the_window_tagged_by_state() {
    let json = |r: Readiness| serde_json::to_value(r).unwrap();
    assert_eq!(json(Readiness::Ready { notes: vec![] }), serde_json::json!({ "state": "ready", "notes": [] }));
    assert_eq!(json(Readiness::Wait { waiting_for: vec!["x".into()] }), serde_json::json!({ "state": "wait", "waitingFor": ["x"] }));
    assert_eq!(json(Readiness::Ask { runs: vec!["p".into()] }), serde_json::json!({ "state": "ask", "runs": ["p"] }));
    let choice: RunsChoice = serde_json::from_str("\"stop\"").unwrap();
    assert_eq!(choice, RunsChoice::Stop);
}

// ---- when a check is due -------------------------------------------------------------------------------------

#[test]
fn checks_happen_soon_after_starting_then_every_few_hours_and_never_during_a_sync() {
    assert!(FIRST_CHECK.as_secs() < 60);
    assert!((2 * 3600..=12 * 3600).contains(&EVERY.as_secs()), "every few hours, not {EVERY:?}");
    assert!(SYNC_WAIT < EVERY);

    assert_eq!(when_due(true, false), Due::Check);
    assert_eq!(when_due(true, true), Due::AfterSync);
    // Turned off: nothing is asked, syncing or not.
    assert_eq!(when_due(false, false), Due::Skip);
    assert_eq!(when_due(false, true), Due::Skip);
}

// ---- what the notice shows -----------------------------------------------------------------------------------

#[test]
fn a_major_release_is_one_whose_first_number_goes_up() {
    assert!(is_major("0.4.2", "1.0.0"));
    assert!(is_major("v1.9.0", "v2.0.0"));
    assert!(is_major("1.2.3", "2.0.0-beta.1"));
    for (from, to) in [("0.4.2", "0.5.0"), ("0.4.2", "0.4.3"), ("1.2.3", "1.3.0"), ("2.0.0", "1.9.9"), ("1.0.0", "1.0.0"), ("nonsense", "2.0.0"), ("1.0.0", "")] {
        assert!(!is_major(from, to), "{from} → {to}");
    }
}

/// What `scripts/release.mjs` writes: the breaking changes first, then features, fixes and the rest.
const NOTES: &str = "### Breaking changes

- **sync:** re-enter your GitHub token in Settings ([a1b2c3d](https://github.com/openledger-nl/ledger/commit/a1b2c3d))

### Features

- **board:** drag cards between columns ([d4e5f6a](https://github.com/openledger-nl/ledger/commit/d4e5f6a))

### Fixes

- **search:** find words with accents ([f0a1b2c](https://github.com/openledger-nl/ledger/commit/f0a1b2c))";

#[test]
fn the_breaking_changes_are_taken_apart_from_the_other_notes() {
    let notes = split_notes(NOTES);
    assert_eq!(notes.breaking.as_deref(), Some("- **sync:** re-enter your GitHub token in Settings ([a1b2c3d](https://github.com/openledger-nl/ledger/commit/a1b2c3d))"));
    assert!(notes.rest.starts_with("### Features"), "{}", notes.rest);
    assert!(notes.rest.contains("### Fixes") && !notes.rest.contains("Breaking"));

    // Anywhere in the notes, any heading level, any capitals; a deeper heading stays inside the section.
    let elsewhere = split_notes("## Fixes\n\n- one\n\n## BREAKING CHANGES ##\n\n- two\n\n### Details\n\nthree\n\n## Other\n\n- four");
    assert_eq!(elsewhere.breaking.as_deref(), Some("- two\n\n### Details\n\nthree"));
    assert_eq!(elsewhere.rest, "## Fixes\n\n- one\n\n## Other\n\n- four");

    // None at all, or only a heading in a code block: nothing stands out.
    assert_eq!(split_notes("### Fixes\n\n- one").breaking, None);
    let fenced = split_notes("```\n### Breaking changes\n```\n- one");
    assert_eq!(fenced.breaking, None);
    assert!(fenced.rest.contains("### Breaking changes"));
    assert_eq!(split_notes("### Breaking changes\n\n### Features\n\n- one").breaking, None);
    assert_eq!(split_notes(""), Default::default());
}

#[test]
fn the_notice_gets_the_notes_as_safe_html_with_the_breaking_changes_apart() {
    let info = update_info("v1.0.0", "0.4.2", Some("2026-11-03T10:00:00Z"), Some(NOTES), false);
    assert_eq!((info.version.as_str(), info.current_version.as_str()), ("1.0.0", "0.4.2"));
    assert!(info.major);
    assert_eq!(info.date.as_deref(), Some("2026-11-03T10:00:00Z"));
    let breaking = info.breaking_html.unwrap();
    assert!(breaking.contains("<li><strong>sync:</strong> re-enter your GitHub token"), "{breaking}");
    assert!(!info.notes_html.contains("re-enter"));
    assert!(info.notes_html.contains("<h3>Features</h3>"));

    // A minor release without notes, and notes that try to inject markup.
    let plain = update_info("0.5.0", "0.4.2", Some(" "), None, false);
    assert!(!plain.major && plain.breaking_html.is_none() && plain.notes_html.is_empty() && plain.date.is_none());
    let hostile = update_info("0.5.0", "0.4.2", None, Some("<script>alert(1)</script>\n\n[x](javascript:alert(1))"), false);
    assert!(!hostile.notes_html.contains("<script>") && !hostile.notes_html.contains("javascript:"), "{}", hostile.notes_html);
}

#[test]
fn a_development_build_pretends_a_minor_or_a_major_release() {
    assert_eq!(simulated_version("0.4.0", false), "0.5.0");
    assert_eq!(simulated_version("0.4.7", true), "1.0.0");
    assert_eq!(simulated_version("v2.3.1-beta", false), "2.4.0");

    let minor = update_info(&simulated_version("0.4.0", false), "0.4.0", None, Some(&simulated_notes(false)), true);
    assert!(minor.simulated && !minor.major && minor.breaking_html.is_none());
    let major = update_info(&simulated_version("0.4.0", true), "0.4.0", None, Some(&simulated_notes(true)), true);
    assert!(major.major && major.breaking_html.is_some());
}

// ---- the key, and the config that ships ------------------------------------------------------------------------

fn tauri_conf() -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json")).unwrap()).unwrap()
}

#[test]
fn only_a_real_minisign_public_key_lets_a_build_look_for_updates() {
    // What `tauri signer generate` prints: a minisign public key file, in base64.
    let real = STANDARD.encode("untrusted comment: minisign public key: 1A2B3C4D5E6F7A8B\nRWSLe1Q2b3c4d5e6f7g8h9i0j1k2l3m4n5o6p7q8r9s0t1u2v3w4x5y6\n");
    assert_eq!(key_problem(Some(&real)), None);
    assert_eq!(key_problem(Some(&format!("  {real}\n"))), None);

    for missing in [None, Some(""), Some("   "), Some("REPLACE-WITH-THE-UPDATER-PUBLIC-KEY"), Some("bm90IGEga2V5"), Some("%%%")] {
        assert!(key_problem(missing).is_some(), "{missing:?}");
    }
}

#[test]
fn the_app_asks_the_latest_github_release_and_local_builds_need_no_signing_key() {
    let conf = tauri_conf();
    let updater = &conf["plugins"]["updater"];
    assert_eq!(updater["endpoints"], serde_json::json!(["https://github.com/openledger-nl/ledger/releases/latest/download/latest.json"]));
    assert!(updater["pubkey"].is_string(), "the public key (or its placeholder) belongs in tauri.conf.json");
    // An update must be signed for the version the release announces, so an old release can't be passed off as new.
    assert_eq!(updater["requireSignedVersion"], serde_json::json!(true));
    // Nothing insecure in the shipped config.
    for key in ["dangerousInsecureTransportProtocol", "dangerousAcceptInvalidCerts", "dangerousAcceptInvalidHostnames", "allowDowngrades"] {
        assert!(updater.get(key).is_none_or(|v| v == false), "{key}");
    }

    // Signing the updater files needs the private key, which only CI has: a contributor's `pnpm tauri build` must not
    // try. CI turns it on with its own config file.
    assert!(conf["bundle"].get("createUpdaterArtifacts").is_none_or(|v| v == false));
    let ci: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.release.conf.json")).unwrap()).unwrap();
    assert_eq!(ci["bundle"]["createUpdaterArtifacts"], serde_json::json!(true));
    // Ad-hoc, not a paid identity: the macOS app starts on Apple silicon after the Privacy & Security step.
    assert_eq!(ci["bundle"]["macOS"]["signingIdentity"], serde_json::json!("-"));
}

// ---- where the choice is kept ----------------------------------------------------------------------------------

#[test]
fn automatic_checks_are_on_unless_turned_off_on_this_machine() {
    let dir = tempdir().unwrap();
    assert_eq!(settings::load_config(dir.path()).updates, UpdateConfig { automatic: true });

    // A config from before updates existed keeps its vault and checks automatically.
    fs::write(dir.path().join("config.json"), r#"{ "vaultPath": "D:\\Notes" }"#).unwrap();
    let old = settings::load_config(dir.path());
    assert_eq!((old.vault_path.as_deref(), old.updates.automatic), (Some("D:\\Notes"), true));

    settings::update_config(dir.path(), |c| c.updates.automatic = false).unwrap();
    let saved = settings::load_config(dir.path());
    assert_eq!((saved.vault_path.as_deref(), saved.updates.automatic), (Some("D:\\Notes"), false));
    let text = fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(text.contains("\"updates\"") && text.contains("\"automatic\": false"), "{text}");

    // Edited into nonsense by hand: the default, and the vault's location is not lost with it.
    fs::write(dir.path().join("config.json"), r#"{ "vaultPath": "D:\\Notes", "updates": { "automatic": "sometimes" } }"#).unwrap();
    let damaged = settings::load_config(dir.path());
    assert_eq!((damaged.vault_path.as_deref(), damaged.updates.automatic), (Some("D:\\Notes"), true));
}

// ---- a refused update, in words -------------------------------------------------------------------------------

#[test]
fn an_update_with_a_wrong_or_missing_signature_is_refused_in_plain_words() {
    use tauri_plugin_updater::Error as E;
    // Signed with another key, or changed after signing (the plugin's minisign errors), or not a signature at all.
    let wrong = install_failure(&E::SignatureUtf8("not base64".into()));
    assert!(wrong.contains("refused") && wrong.contains("doesn't match Ledger's key") && wrong.contains("Nothing was installed"), "{wrong}");
    // A genuine older release announced as a newer version: refused because the signature names its real version.
    for older in [E::MissingSignedVersion, E::SignedVersionMismatch { signed: "0.4.0".into(), announced: "0.5.0".into() }] {
        let message = install_failure(&older);
        assert!(message.contains("refused") && message.contains("older release") && message.contains("Nothing was installed"), "{message}");
    }
    for refused in [E::MissingSignedVersion, E::SignedVersionMismatch { signed: "0.4.0".into(), announced: "0.5.0".into() }, E::SignatureUtf8("x".into())] {
        assert!(is_signature_error(&refused), "{refused}");
    }
    for other in [E::ReleaseNotFound, E::Network("503".into()), E::InvalidUpdaterFormat] {
        assert!(!is_signature_error(&other), "{other}");
        assert!(!install_failure(&other).contains("signature"));
    }
}
