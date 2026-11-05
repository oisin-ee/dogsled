//! Stable observation endpoints, including state directories beyond Unix socket limits.

use crate::error::{Error, Result};
use sha2::{Digest as _, Sha256};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

/// Both transports share the state identity and validate their private runtime directory
/// on every bind or connect. Short paths preserve their original on-disk names.
pub(crate) fn socket_path(state: &Path, name: &str) -> Result<PathBuf> {
    let ordinary = state.join(name);
    if std::os::unix::net::SocketAddr::from_pathname(&ordinary).is_ok() {
        return Ok(ordinary);
    }
    let directory = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(runtime) => PathBuf::from(runtime).join("dogsled"),
        None => PathBuf::from(format!("/tmp/dogsled-{}", nix::unistd::geteuid())),
    };
    fallback(state, name, &directory)
}

fn fallback(state: &Path, name: &str, directory: &Path) -> Result<PathBuf> {
    if !directory.is_absolute() {
        return Err(Error::Invalid(
            "observation runtime directory must be absolute".into(),
        ));
    }
    match std::fs::DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }
    let metadata = std::fs::symlink_metadata(directory)?;
    validate(&metadata, nix::unistd::geteuid().as_raw())?;
    let state = state.canonicalize()?;
    let hash = Sha256::digest(state.as_os_str().as_bytes());
    let hash: String = hash[..16]
        .iter()
        .flat_map(|byte| {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            [
                char::from(HEX[usize::from(byte >> 4)]),
                char::from(HEX[usize::from(byte & 15)]),
            ]
        })
        .collect();
    let suffix = if name == "observation-wake.sock" {
        "-wake"
    } else {
        ""
    };
    let path = directory.join(format!("{hash}{suffix}.sock"));
    std::os::unix::net::SocketAddr::from_pathname(&path)?;
    Ok(path)
}

fn validate(metadata: &std::fs::Metadata, expected_uid: u32) -> Result<()> {
    if !metadata.is_dir()
        || metadata.uid() != expected_uid
        || metadata.permissions().mode() & 0o7777 != 0o700
    {
        return Err(Error::Invalid(
            "observation runtime directory must be an owned, non-symlink directory with mode 0700"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_state_paths_share_private_endpoints_and_distinguish_wake() {
        let root = std::env::temp_dir().join(crate::ledger::new_id());
        let state = root.join("long".repeat(40));
        std::fs::create_dir_all(&state).unwrap();
        let runtime = root.join("runtime");
        let ordinary = fallback(&state, "observation.sock", &runtime).unwrap();
        let alias = state.join("..").join(state.file_name().unwrap());
        assert_eq!(
            fallback(&alias, "observation.sock", &runtime).unwrap(),
            ordinary
        );
        let wake = fallback(&state, "observation-wake.sock", &runtime).unwrap();
        assert_ne!(wake, ordinary);
        assert_eq!(wake.parent(), ordinary.parent());
        assert_eq!(
            std::fs::metadata(&runtime).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let listener = std::os::unix::net::UnixListener::bind(&ordinary).unwrap();
        let _client = std::os::unix::net::UnixStream::connect(&ordinary).unwrap();
        let datagram = std::os::unix::net::UnixDatagram::bind(&wake).unwrap();
        std::os::unix::net::UnixDatagram::unbound()
            .unwrap()
            .send_to(b"ledger", &wake)
            .unwrap();
        let mut bytes = [0; 6];
        datagram.recv(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ledger");
        drop((listener, datagram));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wrong_owner_runtime_directory_is_refused_without_repair() {
        let directory = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let before = std::fs::symlink_metadata(&directory).unwrap();
        let error = validate(&before, before.uid().checked_add(1).unwrap()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "observation runtime directory must be an owned, non-symlink directory with mode 0700"
        );
        let after = std::fs::symlink_metadata(&directory).unwrap();
        assert_eq!(before.uid(), after.uid());
        assert_eq!(before.permissions().mode(), after.permissions().mode());
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before.dev(), after.dev());
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn unsafe_runtime_directories_are_refused_without_repair() {
        let root = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&root).unwrap();
        let directory = root.join("runtime");
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(fallback(&root, "observation.sock", &directory).is_err());
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o755
        );
        std::fs::remove_dir(&directory).unwrap();
        std::os::unix::fs::symlink(&root, &directory).unwrap();
        assert!(fallback(&root, "observation.sock", &directory).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
