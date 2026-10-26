//! The portal's own token.
//!
//! The token is a random value in a file that only the user can read. Later
//! steps of the portal use it to authenticate the calls that change state. No
//! route reads it yet.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::ledger::Ledger;

/// The bytes of a token.
const TOKEN_BYTES: usize = 32;

/// The file name of the token, under the ledger's `web` directory.
const TOKEN_FILE: &str = "token";

/// The path of the token file.
#[must_use]
pub fn path(ledger: &Ledger) -> PathBuf {
    ledger.dir.join("web").join(TOKEN_FILE)
}

/// The portal's token, read from its file.
///
/// The file is created the first time with random bytes and mode `0600`. Later
/// calls read the same token, so the portal keeps working across restarts.
pub fn token(ledger: &Ledger) -> Result<String> {
    let path = path(ledger);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let token = text.trim().to_owned();
            if token.is_empty() {
                return Err(Error::Invalid(format!(
                    "web token file {} is empty",
                    path.display()
                )));
            }
            Ok(token)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => create(&path),
        Err(error) => Err(error.into()),
    }
}

/// Write a new token, readable by its owner only.
fn create(path: &Path) -> Result<String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes = [0_u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|error| Error::Invalid(format!("random token: {error}")))?;
    let token = hex(&bytes);
    // `create_new` keeps a token another process wrote between the read and
    // this write; that token wins and this one is dropped.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(token.as_bytes())?;
            file.sync_all()?;
            Ok(token)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let text = std::fs::read_to_string(path)?;
            Ok(text.trim().to_owned())
        }
        Err(error) => Err(error.into()),
    }
}

/// The lowercase hexadecimal form of the bytes.
fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        text.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_token_file_is_private_and_reused() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let path = path(&ledger);
        let token = super::token(&ledger).expect("a token");
        assert_eq!(token.len(), TOKEN_BYTES * 2);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));

        let mode = std::fs::metadata(&path)
            .expect("the token file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        assert_eq!(super::token(&ledger).expect("the same token"), token);
        assert_eq!(
            std::fs::read_to_string(&path).expect("the token file"),
            token
        );
    }

    #[test]
    fn two_ledgers_hold_different_tokens() {
        let first = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let second = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        assert_ne!(
            token(&first).expect("a token"),
            token(&second).expect("a token")
        );
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xf0, 0xff]), "000ff0ff");
    }
}
