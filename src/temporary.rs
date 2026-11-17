//! Disposable state has a scoped owner. Only this private namespace is orphan-swept.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct Directory {
    entry: PathBuf,
    path: PathBuf,
    lease: File,
}

impl std::ops::Deref for Directory {
    type Target = Path;

    fn deref(&self) -> &Path {
        self.path()
    }
}

impl AsRef<Path> for Directory {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

pub(crate) fn root() -> PathBuf {
    // Keep Unix socket paths short on Darwin too. Never scan arbitrary r-* directories.
    Path::new("/tmp").join(format!("dogsled-tmp-v1-{}", nix::unistd::geteuid()))
}

fn private_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::other(
            "temporary directory is not private and owned",
        ));
    }
    Ok(())
}

impl Directory {
    pub(crate) fn new() -> io::Result<Self> {
        Self::in_root(&root())
    }

    fn in_root(root: &Path) -> io::Result<Self> {
        match fs::DirBuilder::new().mode(0o700).create(root) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
        private_directory(root)?;
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
        let mut name = format!("{}-", std::process::id());
        for byte in random {
            write!(name, "{byte:02x}").map_err(io::Error::other)?;
        }
        let entry = root.join(name);
        fs::DirBuilder::new().mode(0o700).create(&entry)?;
        let setup = || {
            let lease = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(entry.join("lease"))?;
            lease.lock()?;
            let path = entry.join("data");
            fs::DirBuilder::new().mode(0o700).create(&path)?;
            Ok(Self {
                entry: entry.clone(),
                path,
                lease,
            })
        };
        match setup() {
            Ok(directory) => Ok(directory),
            Err(error) => {
                if let Err(cleanup) = fs::remove_dir_all(&entry) {
                    eprintln!("dogsled temporary cleanup: {}: {cleanup}", entry.display());
                }
                Err(error)
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.entry)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "dogsled temporary cleanup: {}: {error}",
                self.entry.display()
            );
        }
        // Also release any descriptor duplicated before an exec.
        if let Err(error) = self.lease.unlock() {
            eprintln!(
                "dogsled temporary unlock: {}: {error}",
                self.entry.display()
            );
        }
    }
}

fn dead_owner(name: &str) -> bool {
    let Some((pid, nonce)) = name.split_once('-') else {
        return false;
    };
    if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    let Ok(pid) = pid.parse::<i32>() else {
        return false;
    };
    // PID liveness closes the create-directory / acquire-lease race. PID reuse only
    // postpones collection; age or an unavailable process table never proves death.
    pid > 1
        && nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None)
            == Err(nix::errno::Errno::ESRCH)
}

pub(crate) fn sweep(root: &Path) -> io::Result<usize> {
    match private_directory(root) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    }
    let mut removed = 0;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_name().to_str().is_some_and(dead_owner) {
            continue;
        }
        let path = entry.path();
        if private_directory(&path).is_err() {
            continue;
        }
        let lease = match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(path.join("lease"))
        {
            Ok(lease) => lease,
            // An interrupted allocation before lease creation has no payload. Its dead
            // owner is sufficient, but remove_dir refuses any unexpected content.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::remove_dir(&path) {
                    Ok(()) => removed += 1,
                    Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => (),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error),
                }
                continue;
            }
            Err(_) => continue,
        };
        let metadata = lease.metadata()?;
        if !metadata.is_file() || metadata.uid() != nix::unistd::geteuid().as_raw() {
            continue;
        }
        match lease.try_lock() {
            Ok(()) => (),
            Err(std::fs::TryLockError::WouldBlock) => continue,
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
        // Another collector may have removed this entry while we waited.
        let current = match fs::symlink_metadata(path.join("lease")) {
            Ok(current) => current,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if current.dev() == metadata.dev() && current.ino() == metadata.ino() {
            fs::remove_dir_all(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Child(std::process::Child);

    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            self.0.wait().unwrap();
        }
    }

    #[test]
    fn scoped_directory_cleans_success_and_unwind() {
        let directory = Directory::new().unwrap();
        let path = directory.path().to_owned();
        fs::write(path.join("state.db"), b"test").unwrap();
        drop(directory);
        assert!(!path.exists());
        let directory = Directory::new().unwrap();
        let path = directory.path().to_owned();
        let result = std::panic::catch_unwind(move || {
            let _owner = directory;
            panic!("fixture failure");
        });
        assert!(result.is_err());
        assert!(!path.exists());
    }

    #[test]
    fn orphan_child() {
        use std::io::Write as _;
        let Some(root) = std::env::var_os("DOGSLED_TEMP_ORPHAN_TEST") else {
            return;
        };
        let directory = Directory::in_root(Path::new(&root)).unwrap();
        fs::write(directory.path().join("state.db"), b"test").unwrap();
        // Parent kills this isolated process. No destructor can run.
        println!("{}", directory.path().display());
        io::stdout().flush().unwrap();
        std::thread::park();
    }

    #[test]
    fn sweep_requires_dead_owner_and_an_unheld_lease() {
        use std::io::BufRead as _;
        let parent = Directory::new().unwrap();
        let root = parent.path().join("owners");
        let active = Directory::in_root(&root).unwrap();
        let unrelated = root.join("r-123-456-789");
        fs::create_dir_all(&unrelated).unwrap();
        fs::write(unrelated.join("keep"), b"unmarked").unwrap();
        let mut child = Child(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "temporary::tests::orphan_child", "--nocapture"])
                .env("DOGSLED_TEMP_ORPHAN_TEST", &root)
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut output = io::BufReader::new(child.0.stdout.take().unwrap());
        let mut line = String::new();
        let orphan = loop {
            line.clear();
            assert!(output.read_line(&mut line).unwrap() > 0);
            if line.starts_with('/') {
                break PathBuf::from(line.trim());
            }
        };
        assert_eq!(sweep(&root).unwrap(), 0);
        let lease = File::options()
            .read(true)
            .write(true)
            .open(orphan.parent().unwrap().join("lease"))
            .unwrap();
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        lease.lock().unwrap();
        assert_eq!(sweep(&root).unwrap(), 0);
        lease.unlock().unwrap();
        assert_eq!(sweep(&root).unwrap(), 1);
        assert!(!orphan.exists());
        std::os::unix::fs::symlink(active.path(), orphan.parent().unwrap()).unwrap();
        assert_eq!(sweep(&root).unwrap(), 0);
        assert!(active.path().exists());
        assert!(unrelated.join("keep").exists());
        assert_eq!(sweep(&root).unwrap(), 0);
    }
}
