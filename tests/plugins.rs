//! Package contracts, including the commands that the harnesses actually run.
#![allow(clippy::unwrap_used, clippy::panic)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn json(path: &str) -> Value {
    serde_json::from_slice(&std::fs::read(root().join(path)).unwrap()).unwrap()
}

fn toml(path: &str) -> toml::Value {
    toml::from_str(&std::fs::read_to_string(root().join(path)).unwrap()).unwrap()
}

fn relative(base: &Path, value: &Value) -> PathBuf {
    let path = value.as_str().unwrap();
    assert!(path.starts_with("./"), "{path}");
    assert!(!path.split('/').any(|part| part == ".."));
    let target = base.join(path);
    assert!(target.exists(), "{}", target.display());
    target
}

#[test]
fn all_package_versions_match_cargo_without_a_release_bump() {
    let cargo = toml("Cargo.toml");
    let version = cargo["package"]["version"].as_str().unwrap();
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
    for manifest in [
        "package.json",
        "package-lock.json",
        "plugins/dogsled/.claude-plugin/plugin.json",
        "plugins/dogsled/.codex-plugin/plugin.json",
    ] {
        assert_eq!(json(manifest)["version"], version, "{manifest}");
    }
    assert_eq!(
        toml("herdr-plugin.toml")["version"].as_str().unwrap(),
        version
    );
    assert_eq!(
        json("package-lock.json")["packages"][""]["version"],
        version
    );
}

#[test]
fn marketplaces_and_manifests_resolve_the_single_skill_and_hooks() {
    for (marketplace, manifest, codex) in [
        (
            ".claude-plugin/marketplace.json",
            ".claude-plugin/plugin.json",
            false,
        ),
        (
            ".agents/plugins/marketplace.json",
            ".codex-plugin/plugin.json",
            true,
        ),
    ] {
        let market = json(marketplace);
        assert_eq!(market["name"], "dogsled");
        let plugins = market["plugins"].as_array().unwrap();
        assert_eq!(plugins.len(), 1);
        let entry = &plugins[0];
        assert_eq!(entry["name"], "dogsled");
        let source = if codex {
            &entry["source"]["path"]
        } else {
            &entry["source"]
        };
        assert_eq!(source, "./plugins/dogsled");
        let plugin = relative(&root(), source);
        let metadata = json(&format!("plugins/dogsled/{manifest}"));
        assert_eq!(metadata["name"], entry["name"]);
        let skills = if codex {
            relative(&plugin, &metadata["skills"])
        } else {
            plugin.join("skills")
        };
        assert!(skills.join("dogsled/SKILL.md").is_file());
        assert_eq!(std::fs::read_dir(&skills).unwrap().count(), 1);
        assert!(plugin.join("hooks/context.sh").is_file());
        assert!(plugin.join("hooks/inbox.sh").is_file());
        let hooks = if codex {
            metadata["hooks"].clone()
        } else {
            json("plugins/dogsled/hooks/hooks.json")
        };
        assert_eq!(hooks["hooks"].as_object().unwrap().len(), 3);
        let (harness, variable) = if codex {
            ("codex", "$PLUGIN_ROOT")
        } else {
            ("claude", "$CLAUDE_PLUGIN_ROOT")
        };
        // Context at session start and each prompt; the inbox at each prompt and each stop.
        for (event, index, script) in [
            ("SessionStart", 0, "context"),
            ("UserPromptSubmit", 0, "context"),
            ("UserPromptSubmit", 1, "inbox"),
            ("Stop", 0, "inbox"),
        ] {
            let handler = &hooks["hooks"][event][0]["hooks"][index];
            assert_eq!(handler["type"], "command");
            assert_eq!(handler["timeout"], 6);
            let command = handler["command"].as_str().unwrap();
            assert_eq!(
                command,
                format!(
                    "sh \"{variable}/hooks/{script}.sh\" --harness {harness} --event {event} || true"
                )
            );
            // Empty PATH means no dogsled. An absolute shell path lets the
            // registered command run, including its missing-binary guard.
            let command = command.replacen("sh ", "/bin/sh ", 1);
            let output = Command::new("/bin/sh")
                .args(["-c", &command])
                .env("PATH", "")
                .env("PLUGIN_ROOT", &plugin)
                .env("CLAUDE_PLUGIN_ROOT", &plugin)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, [] as [u8; 0]);
            assert_eq!(output.stderr, [] as [u8; 0]);
        }
    }
    let package = json("package.json");
    let skills = package["pi"]["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 1);
    assert!(relative(&root(), &skills[0]).join("SKILL.md").is_file());
    assert!(!root().join("skills/dogsled").exists());
    for extension in package["pi"]["extensions"].as_array().unwrap() {
        relative(&root(), extension);
    }
}

#[test]
fn claude_and_codex_plugins_start_dogsled_mcp_with_the_callers_pane() {
    let codex = json("plugins/dogsled/.codex-plugin/plugin.json");
    let path = codex["mcpServers"].as_str().unwrap();
    for (servers, forwards) in [
        (json("plugins/dogsled/.mcp.json"), false),
        (
            json(&format!(
                "plugins/dogsled/{}",
                path.trim_start_matches("./")
            )),
            true,
        ),
    ] {
        let server = &servers["mcpServers"]["dogsled"];
        assert_eq!(
            (&server["command"], &server["args"]),
            (&serde_json::json!("dogsled"), &serde_json::json!(["mcp"]))
        );
        // Codex forwards only the variables it names, and the server needs the caller's pane.
        if forwards {
            let forwarded = server["env_vars"].as_array().unwrap();
            for name in [
                "HERDR_ENV",
                "HERDR_PANE_ID",
                "HERDR_WORKSPACE_ID",
                "HERDR_SOCKET_PATH",
            ] {
                assert!(forwarded.contains(&serde_json::json!(name)), "{name}");
            }
        }
    }
}

#[test]
fn dist_keeps_service_templates_and_herdr_keeps_its_board() {
    let dist = toml("dist-workspace.toml");
    let includes = dist["dist"]["include"].as_array().unwrap();
    for path in [
        "plugin/services/dogsled.service",
        "plugin/services/dev.dogsled.plist",
    ] {
        assert!(includes.iter().any(|value| value.as_str() == Some(path)));
        assert!(root().join(path).is_file());
    }
    let herdr = toml("herdr-plugin.toml");
    assert_eq!(herdr["id"].as_str(), Some("dogsled"));
    assert!(
        root()
            .join(herdr["panes"][0]["command"][1].as_str().unwrap())
            .is_file()
    );
}
