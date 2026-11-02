//! Sealing, opening and keeping keys. Keys go into memory, or into the `.env` of a temporary vault.

use std::fs;
use std::io::Write;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use flate2::Compression;
use flate2::write::DeflateEncoder;
use tempfile::tempdir;

use crate::crypto::{self, CryptoError, Key, KeyError};
use crate::keystore::{self, EnvFileKeyStore, KeyStore, MemoryKeyStore};
use crate::settings;
use crate::sync::MAX_FILE_BYTES;

fn key(byte: u8) -> Key {
    Key::from_bytes([byte; 32])
}

/// A key whose label differs from `other`'s, so that "a different key" is recognised by its header.
fn different_from(other: &Key) -> Key {
    (1u8..=255).map(key).find(|k| k.id() != other.id()).unwrap()
}

// ---- sealing and opening ----------------------------------------------------------------------------

#[test]
fn a_file_survives_the_round_trip() {
    let key = key(1);
    for plaintext in [&b""[..], b"hello", "naïve — ünïcode ✓".as_bytes(), &[0u8, 159, 146, 150, 255], &vec![42u8; 100_000]] {
        let sealed = crypto::encrypt(&key, "projects/p/readme.md", plaintext).unwrap();
        assert_eq!(crypto::decrypt(&key, "projects/p/readme.md", &sealed).unwrap(), plaintext);
    }
}

#[test]
fn what_is_sealed_looks_nothing_like_the_file_and_is_new_every_time() {
    let key = key(2);
    let text = b"the quick brown fox jumps over the lazy dog, again and again and again";
    let (a, b) = (crypto::encrypt(&key, "a.md", text).unwrap(), crypto::encrypt(&key, "a.md", text).unwrap());
    assert_ne!(a, b, "a fresh nonce for every encryption");
    assert!(!a.windows(text.len().min(12)).any(|w| w == &text[..w.len()]), "no plaintext shows through");
    assert_eq!(&a[..4], &[0x89, b'L', b'D', b'G'], "magic");
    assert_eq!(a[4], 1, "format version");
    assert_eq!(a[5], key.id(), "key id");
}

#[test]
fn files_are_compressed_before_they_are_sealed() {
    let key = key(3);
    let text = "the same line over and over\n".repeat(2_000);
    let sealed = crypto::encrypt(&key, "notes.md", text.as_bytes()).unwrap();
    assert!(sealed.len() < text.len() / 10, "{} bytes sealed from {}", sealed.len(), text.len());
}

#[test]
fn the_wrong_key_is_recognised_and_says_which_key_it_was() {
    let (right, wrong) = (key(4), different_from(&key(4)));
    let sealed = crypto::encrypt(&right, "a.md", b"secret").unwrap();
    assert_eq!(crypto::decrypt(&wrong, "a.md", &sealed), Err(CryptoError::WrongKey { found: right.id(), expected: wrong.id() }));
}

#[test]
fn a_key_with_the_same_label_still_cannot_open_it() {
    // Labels are one byte, so two keys can share one; the cipher itself must still say no.
    let right = key(5);
    let twin = (0u32..100_000)
        .map(|n| {
            let mut bytes = [0u8; 32];
            bytes[..4].copy_from_slice(&n.to_le_bytes());
            Key::from_bytes(bytes)
        })
        .find(|k| k.id() == right.id())
        .expect("some other key shares the label");
    let sealed = crypto::encrypt(&right, "a.md", b"secret").unwrap();
    assert_eq!(crypto::decrypt(&twin, "a.md", &sealed), Err(CryptoError::Failed));
}

#[test]
fn a_tampered_file_is_refused_wherever_it_was_touched() {
    let key = key(6);
    let sealed = crypto::encrypt(&key, "a.md", b"some words worth protecting").unwrap();

    // Every byte after the header: nonce, ciphertext, tag.
    for i in 6..sealed.len() {
        let mut bad = sealed.clone();
        bad[i] ^= 0x01;
        assert_eq!(crypto::decrypt(&key, "a.md", &bad), Err(CryptoError::Failed), "byte {i}");
    }

    let mut other_version = sealed.clone();
    other_version[4] = 2;
    assert_eq!(crypto::decrypt(&key, "a.md", &other_version), Err(CryptoError::UnsupportedVersion(2)));

    let mut other_label = sealed.clone();
    other_label[5] ^= 0x01;
    assert!(matches!(crypto::decrypt(&key, "a.md", &other_label), Err(CryptoError::WrongKey { .. })));

    assert_eq!(crypto::decrypt(&key, "a.md", &sealed[..sealed.len() - 1]), Err(CryptoError::Failed), "a clipped tag");
    assert_eq!(crypto::decrypt(&key, "a.md", &sealed[..10]), Err(CryptoError::Corrupt), "cut short");
    assert!(crypto::decrypt(&key, "a.md", &sealed).is_ok(), "the original is untouched");
}

#[test]
fn a_file_moved_to_another_path_no_longer_opens() {
    let key = key(7);
    let sealed = crypto::encrypt(&key, "projects/a/readme.md", b"for a").unwrap();
    assert_eq!(crypto::decrypt(&key, "projects/b/readme.md", &sealed), Err(CryptoError::Failed));
    assert_eq!(crypto::decrypt(&key, "projects/a/readme.md", &sealed).unwrap(), b"for a");
}

#[test]
fn a_plain_file_is_not_mistaken_for_a_sealed_one() {
    let key = key(8);
    assert_eq!(crypto::decrypt(&key, "a.md", b"# just markdown"), Err(CryptoError::NotEncrypted));
    assert_eq!(crypto::decrypt(&key, "a.md", b""), Err(CryptoError::NotEncrypted));
}

#[test]
fn a_decompression_bomb_is_stopped_and_so_is_an_oversized_file() {
    let bomb = {
        let mut packer = DeflateEncoder::new(Vec::new(), Compression::best());
        packer.write_all(&vec![0u8; 1_000_000]).unwrap();
        packer.finish().unwrap()
    };
    assert!(bomb.len() < 5_000, "a megabyte of zeros packs small");
    assert_eq!(crypto::inflate_for_test(&bomb, 999_999), Err(CryptoError::TooLarge));
    assert_eq!(crypto::inflate_for_test(&bomb, 1_000_000).unwrap().len(), 1_000_000);

    let too_big = vec![0u8; MAX_FILE_BYTES as usize + 1];
    assert_eq!(crypto::encrypt(&key(9), "huge.bin", &too_big), Err(CryptoError::TooLarge));
}

#[test]
fn the_stored_form_is_text_and_only_text_that_starts_like_it_counts_as_sealed() {
    let sealed = crypto::encrypt(&key(10), "a.md", b"hello").unwrap();
    let stored = crypto::armor(&sealed);
    assert!(std::str::from_utf8(&stored).is_ok(), "stored as text, so GitHub takes it inside the tree request");
    assert!(crypto::is_encrypted(&stored));
    assert_eq!(crypto::unarmor(&stored).unwrap(), sealed);
    assert_eq!(crypto::unarmor(&[stored.clone(), b"\n".to_vec()].concat()).unwrap(), sealed, "a trailing newline is tolerated");

    for k in 1..=40 {
        assert!(crypto::is_encrypted(&crypto::armor(&crypto::encrypt(&key(k), "x", b"y").unwrap())), "whatever the key");
    }
    for plain in [&b""[..], b"# a note", b"iUx", b"----\ntitle: x\n---\n", &[0x89, b'L', b'D', b'G'], &[0u8, 1, 2]] {
        assert!(!crypto::is_encrypted(plain));
        assert_eq!(crypto::unarmor(plain), Err(CryptoError::NotEncrypted));
    }
    let mut damaged = stored.clone();
    damaged.truncate(damaged.len() - 3);
    damaged.extend_from_slice(b"!!!");
    assert_eq!(crypto::unarmor(&damaged), Err(CryptoError::Corrupt));
}

// ---- keys as text -----------------------------------------------------------------------------------

#[test]
fn a_recovery_string_brings_the_same_key_back() {
    let key = Key::generate();
    let recovery = key.to_recovery();
    assert!(recovery.starts_with("ledger-key-1:"));
    let back = Key::parse(&recovery).unwrap();
    assert_eq!(back.as_bytes(), key.as_bytes());
    assert_eq!(back.id(), key.id());

    // Pasting adds spaces and line breaks.
    let wrapped = format!("  {}\n{}  \n", &recovery[..20], &recovery[20..]);
    assert_eq!(Key::parse(&wrapped).unwrap().as_bytes(), key.as_bytes());
}

#[test]
fn a_bare_key_is_accepted_as_hex_or_base64() {
    let key = key(0xab);
    let bytes = key.as_bytes();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(Key::parse(&hex).unwrap().as_bytes(), bytes);
    assert_eq!(Key::parse(&hex.to_uppercase()).unwrap().as_bytes(), bytes);
    for encoded in [STANDARD.encode(bytes), STANDARD.encode(bytes).trim_end_matches('=').to_string(), URL_SAFE_NO_PAD.encode(bytes)] {
        assert_eq!(Key::parse(&encoded).unwrap().as_bytes(), bytes, "{encoded}");
    }
}

#[test]
fn text_that_is_not_the_key_is_refused_without_repeating_it() {
    let recovery = key(11).to_recovery();

    // One character changed: the checksum notices.
    let mut typo: Vec<char> = recovery.chars().collect();
    let at = typo.len() - 8;
    typo[at] = if typo[at] == 'A' { 'B' } else { 'A' };
    assert_eq!(Key::parse(&typo.into_iter().collect::<String>()).err(), Some(KeyError::BadChecksum));

    assert_eq!(Key::parse(&recovery[..recovery.len() - 6]).err(), Some(KeyError::Malformed), "cut short");
    assert_eq!(Key::parse("ledger-key-9:AAAA").err(), Some(KeyError::UnsupportedVersion));
    for junk in ["", "hello world", "ledger-key-1:", "0123456789", &"g".repeat(64)] {
        let error = Key::parse(junk).err().unwrap_or_else(|| panic!("{junk:?} was accepted"));
        assert!(!error.to_string().contains(junk) || junk.is_empty(), "the message must not echo what was typed");
    }
}

#[test]
fn a_key_never_shows_in_debug_output() {
    let key = key(0x5c);
    let recovery = key.to_recovery();
    let hex: String = key.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let shown = format!("{key:?} {:?}", Some(&key));
    assert!(!shown.contains(&hex) && !shown.contains(&recovery[13..]) && !shown.contains("92, 92, 92"), "{shown}");
    assert!(shown.contains("Key(id"), "{shown}");
}

// ---- where the key lives ----------------------------------------------------------------------------

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_default()
}

/// Every file under `root`, with its path.
fn all_files(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push((path.display().to_string(), fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = vec![];
    walk(root, &mut out);
    out
}

#[test]
fn the_key_is_one_line_of_the_env_file_and_the_other_lines_are_left_alone() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let keys = EnvFileKeyStore;
    assert!(keys.load(root).unwrap().is_none(), "no file, no key");

    // A token and a line of someone's own are already there.
    fs::create_dir_all(root.join(".ledger")).unwrap();
    fs::write(root.join(".ledger/.env"), "# mine\nOTHER=keep me\nGITHUB_TOKEN=tok\n").unwrap();
    assert!(keys.load(root).unwrap().is_none(), "a token is not a key");

    keys.save(root, &key(7)).unwrap();
    assert_eq!(keys.load(root).unwrap().unwrap().to_recovery().to_string(), key(7).to_recovery().to_string());
    let text = read(root, ".ledger/.env");
    assert!(text.contains("# mine") && text.contains("OTHER=keep me") && text.contains("GITHUB_TOKEN=tok"), "{text}");

    // Saving again replaces the one line; saving the token, or turning sync off, leaves the key.
    keys.save(root, &key(8)).unwrap();
    assert_eq!(read(root, ".ledger/.env").matches("LEDGER_ENCRYPTION_KEY").count(), 1);
    settings::write_env(root, &settings::SyncEnv { repo: "me/notes".into(), branch: "main".into(), token: "new".into() }).unwrap();
    settings::write_env(root, &settings::SyncEnv { repo: "me/notes".into(), branch: "main".into(), token: String::new() }).unwrap();
    assert_eq!(keys.load(root).unwrap().unwrap().id(), key(8).id());
    assert_eq!(settings::read_env(root).token, "");
    assert!(read(root, ".ledger/.env").contains("OTHER=keep me"));
}

#[test]
fn a_damaged_key_line_is_reported_not_used() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut recovery = key(9).to_recovery().to_string();
    let last = recovery.pop().unwrap();
    recovery.push(if last == 'A' { 'B' } else { 'A' });
    fs::create_dir_all(root.join(".ledger")).unwrap();
    fs::write(root.join(".ledger/.env"), format!("LEDGER_ENCRYPTION_KEY={recovery}\n")).unwrap();

    let error = EnvFileKeyStore.load(root).unwrap_err().to_string();
    assert!(error.contains("damaged") && error.contains("recovery key"), "{error}");
    let status = keystore::status(root, &EnvFileKeyStore);
    assert!(!status.has_key && status.problem.is_some(), "{status:?}");
    // A damaged key is as good as none: it can be replaced without asking for more.
    keystore::generate(root, &EnvFileKeyStore, false).unwrap();
    assert!(keystore::status(root, &EnvFileKeyStore).has_key);
}

#[cfg(unix)]
#[test]
fn the_env_file_is_for_its_owner_alone() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    EnvFileKeyStore.save(dir.path(), &key(1)).unwrap();
    let mode = fs::metadata(dir.path().join(".ledger/.env")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
}

#[test]
fn a_key_is_made_kept_exported_and_written_nowhere_but_the_env_file() {
    let dir = tempdir().unwrap();
    let (root, keys) = (dir.path(), EnvFileKeyStore);

    let before = keystore::status(root, &keys);
    assert!((before.enabled, before.has_key, before.key_id.clone()) == (false, false, None), "{before:?}");
    assert!(keystore::export(root, &keys).is_err(), "nothing to export yet");

    keystore::generate(root, &keys, false).unwrap();
    let status = keystore::status(root, &keys);
    assert!(status.has_key && !status.enabled && status.problem.is_none());
    let recovery = keystore::export(root, &keys).unwrap();
    let key = Key::parse(&recovery).unwrap();
    assert_eq!(status.key_id.as_deref(), Some(key.id_hex().as_str()));

    // The key is in `.ledger/.env`, which is gitignored and never synced, and in no other file: not settings.json,
    // not anything that could be uploaded.
    let hex: String = key.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let forms = [recovery.to_string(), recovery[13..].to_string(), hex, STANDARD.encode(key.as_bytes()), URL_SAFE_NO_PAD.encode(key.as_bytes())];
    settings::write_env(root, &settings::SyncEnv { repo: "me/notes".into(), branch: "main".into(), token: "t".into() }).unwrap();
    keystore::set_enabled(root, &keys, true).unwrap();
    for (path, bytes) in all_files(root) {
        let text = String::from_utf8_lossy(&bytes);
        if path.replace('\\', "/").ends_with("/.ledger/.env") {
            assert!(text.contains(forms[0].as_str()), "the key is kept in {path}");
        } else {
            for form in &forms {
                assert!(!text.contains(form.as_str()), "the key is in {path}");
            }
        }
    }
    let settings_json = read(root, ".ledger/settings.json");
    assert!(settings_json.contains("\"enabled\": true"), "settings.json says whether, and nothing more: {settings_json}");
    assert!(crate::sync::Ignore::load(root).is_ignored(".ledger/.env", false), "and the sync never picks it up");

    // What the screen gets to see doesn't contain it either.
    let json = serde_json::to_string(&keystore::status(root, &keys)).unwrap();
    assert!(!forms.iter().any(|f| json.contains(f.as_str())), "{json}");
}

#[test]
fn replacing_a_key_has_to_be_asked_for() {
    let dir = tempdir().unwrap();
    let (root, keys) = (dir.path(), MemoryKeyStore::default());
    keystore::generate(root, &keys, false).unwrap();
    let first = keystore::export(root, &keys).unwrap().to_string();

    assert!(keystore::generate(root, &keys, false).is_err());
    assert!(keystore::import(root, &keys, &key(3).to_recovery(), false).is_err());
    assert_eq!(keystore::export(root, &keys).unwrap().to_string(), first, "refused, so unchanged");

    keystore::import(root, &keys, &key(3).to_recovery(), true).unwrap();
    assert_eq!(keystore::export(root, &keys).unwrap().to_string(), key(3).to_recovery().to_string());
    assert_eq!(keystore::status(root, &keys).key_id, Some(key(3).id_hex()));

    keystore::generate(root, &keys, true).unwrap();
    assert_ne!(keystore::export(root, &keys).unwrap().to_string(), key(3).to_recovery().to_string(), "a new key replaced it");
}

#[test]
fn a_key_moves_to_another_machine_as_its_recovery_string() {
    let (here, there) = (tempdir().unwrap(), tempdir().unwrap());
    let (mine, theirs) = (MemoryKeyStore::default(), MemoryKeyStore::default());
    keystore::generate(here.path(), &mine, false).unwrap();

    assert!(keystore::import(there.path(), &theirs, "nonsense", false).is_err());
    assert!(!keystore::status(there.path(), &theirs).has_key, "a refused import leaves nothing behind");

    keystore::import(there.path(), &theirs, &keystore::export(here.path(), &mine).unwrap(), false).unwrap();
    assert_eq!(keystore::status(there.path(), &theirs).key_id, keystore::status(here.path(), &mine).key_id);
}

#[test]
fn encryption_cannot_be_turned_on_without_a_key_and_a_sync_without_one_stops() {
    let dir = tempdir().unwrap();
    let (root, keys) = (dir.path(), MemoryKeyStore::default());

    assert!(keystore::set_enabled(root, &keys, true).is_err());
    assert!(!keystore::status(root, &keys).enabled);
    assert!(keystore::key_for_sync(root, &keys).unwrap().is_none(), "off: sync as before");

    keystore::generate(root, &keys, false).unwrap();
    keystore::set_enabled(root, &keys, true).unwrap();
    assert!(keystore::key_for_sync(root, &keys).unwrap().is_some());

    // A machine whose `.env` has lost the key, or a vault opened where the key was never imported.
    let elsewhere = MemoryKeyStore::default();
    let error = keystore::key_for_sync(root, &elsewhere).unwrap_err().to_string();
    assert!(error.contains("no key") && error.contains("Import"), "{error}");

    keystore::set_enabled(root, &keys, false).unwrap();
    assert!(keystore::key_for_sync(root, &elsewhere).unwrap().is_none(), "turning it off is always possible");
}

#[test]
fn the_screen_is_told_when_a_branch_cannot_become_an_encrypted_one() {
    let dir = tempdir().unwrap();
    let (root, keys) = (dir.path(), MemoryKeyStore::default());
    let target = |branch: &str| settings::write_env(root, &settings::SyncEnv { repo: "me/notes".into(), branch: branch.into(), token: "t".into() }).unwrap();
    let state = |extra: &str| {
        fs::create_dir_all(root.join(".ledger")).unwrap();
        let sha = "0".repeat(40);
        fs::write(root.join(".ledger/sync-state.json"), format!(r#"{{"repo":"me/notes","branch":"main","head":"c0","files":{{"a.md":"{sha}"}}{extra}}}"#)).unwrap();
    };

    target("main");
    assert!(!keystore::status(root, &keys).needs_fresh_target, "nothing synced yet");

    // This vault has synced main without encryption: main can't become an encrypted branch...
    state("");
    assert!(keystore::status(root, &keys).needs_fresh_target);
    // ...but another branch, or another repository, is a fresh start.
    target("sealed");
    assert!(!keystore::status(root, &keys).needs_fresh_target);
    settings::write_env(root, &settings::SyncEnv { repo: "me/elsewhere".into(), branch: "main".into(), token: "t".into() }).unwrap();
    assert!(!keystore::status(root, &keys).needs_fresh_target);

    // A branch that was encrypted from the start is fine as it is.
    target("main");
    state(r#","encrypted":7"#);
    assert!(!keystore::status(root, &keys).needs_fresh_target);
}
