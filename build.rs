//! The build script of the dogsled binary.
//!
//! The server embeds the Dioxus bundle in `web/public`. `dx bundle --web
//! --release` writes that folder (`moon run :web-build`). The folder is
//! not in the repository, so a build without it must fail with a message
//! that names the command that fills it.

use std::path::Path;

/// The folder the bundle lands in.
const BUNDLE: &str = "web/public";
/// The page the bundle holds.
const INDEX: &str = "index.html";
/// The command that fills the folder.
const BUILD_COMMAND: &str = "moon run :web-build";

fn main() {
    // The bundle changes whenever the web crate changes, so the build must
    // run again when the folder changes.
    println!("cargo:rerun-if-changed={BUNDLE}");
    println!("cargo:rerun-if-changed=web/src");
    println!("cargo:rerun-if-changed=web/Cargo.toml");
    println!("cargo:rerun-if-changed=src/web/assets.rs");

    assert!(
        Path::new(BUNDLE).join(INDEX).is_file(),
        "the web bundle is missing: {BUNDLE}/{INDEX} does not exist.\n\
         Run `{BUILD_COMMAND}` first, which runs `dx bundle --web --release`."
    );
}
