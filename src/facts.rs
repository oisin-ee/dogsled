//! What each agent pane is doing, measured on its own host: the model in its transcript, its
//! latest step, and the state of the linked worktree it runs in. The service publishes these in
//! `host.json`, so every reader sees the same facts without SSH or a transcript of its own.

use crate::activity;
use crate::probe::{Agent, Probe};
use crate::shell::Shell;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Transcript facts. Empty when the transcript is unknown or unreadable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// The model the harness asked for: an alias such as `opus`, or a model id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The model that answered, when the provider reports a different one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// The latest step, on one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
}

impl Session {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// A short model label: `opus 5.5`, `astra→opus 5.5`, `gpt-5.6-sol`, or the alias alone
    /// before the first answer.
    pub fn label(&self) -> Option<String> {
        let asked = self.model.as_deref().map(pretty);
        let answered = self.response_model.as_deref().map(pretty);
        match (asked, answered) {
            (Some(asked), Some(answered)) if answered.split(' ').next() != Some(asked.as_str()) => {
                Some(format!("{asked}→{answered}"))
            }
            (_, Some(answered)) => Some(answered),
            (asked, None) => asked,
        }
    }
}

/// `claude-opus-5-5` → `opus 5.5`; `claude-sonnet-5-20260101` → `sonnet 5`; others unchanged.
fn pretty(model: &str) -> String {
    let Some(rest) = model.strip_prefix("claude-") else {
        return model.to_owned();
    };
    let mut parts = rest.split('-');
    let family = parts.next().unwrap_or(rest);
    let version: Vec<&str> = parts
        .take_while(|p| p.len() <= 2 && p.bytes().all(|b| b.is_ascii_digit()))
        .collect();
    if version.is_empty() {
        family.to_owned()
    } else {
        format!("{family} {}", version.join("."))
    }
}

/// Read `agent`'s transcript facts from `values`, newest first.
pub fn session(agent: &str, values: &[Value]) -> Session {
    let mut session = Session {
        activity: activity::latest(agent, values).map(|step| step.line()),
        ..Session::default()
    };
    match agent {
        "pi" => {
            // The newest assistant message names the answering model; a newer model change
            // means the next answer comes from the new one.
            for value in values {
                if value["type"] == "model_change" {
                    session.model = value["modelId"].as_str().map(str::to_owned);
                    break;
                }
                let message = &value["message"];
                if value["type"] == "message" && message["role"] == "assistant" {
                    session.model = message["model"].as_str().map(str::to_owned);
                    session.response_model = message["responseModel"]
                        .as_str()
                        .filter(|m| Some(*m) != message["model"].as_str())
                        .map(str::to_owned);
                    break;
                }
            }
            session.thinking = values.iter().find_map(|value| {
                if value["type"] == "thinking_level_change" {
                    value["thinkingLevel"].as_str()
                } else if value["type"] == "message" && value["message"]["role"] == "assistant" {
                    value["message"]["thinkingLevel"].as_str()
                } else {
                    None
                }
                .map(str::to_owned)
            });
        }
        "claude" => {
            session.model = values.iter().find_map(|value| {
                (value["type"] == "assistant")
                    .then(|| value["message"]["model"].as_str())
                    .flatten()
                    .filter(|m| !m.starts_with('<'))
                    .map(str::to_owned)
            });
        }
        "codex" => {
            if let Some(context) = values.iter().find(|v| v["type"] == "turn_context") {
                let payload = &context["payload"];
                session.model = payload["model"].as_str().map(str::to_owned);
                session.thinking = payload["effort"].as_str().map(str::to_owned);
            }
        }
        _ => (),
    }
    session
}

/// The linked worktree an agent runs in, and whether its work is safe to remove.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    /// Commits on the branch that the default branch does not contain.
    pub ahead: u64,
    /// Uncommitted or untracked changes.
    pub dirty: bool,
    /// The branch's work is in the default branch (`integration::check`).
    pub landed: bool,
    /// HEAD and the default branch tip when `ahead` and `landed` were measured.
    pub head: String,
    pub base: String,
}

fn git(shell: &dyn Shell, dir: &str, args: &[&str]) -> Option<String> {
    let mut words = vec!["-C".to_string(), dir.to_string()];
    words.extend(args.iter().map(ToString::to_string));
    let out = shell.run("git", &words).ok()?;
    out.ok().then(|| out.stdout.trim().to_owned())
}

/// Measure the worktree at `cwd`. HEAD and the base tip are read every time; the commit count
/// and the landed check run only when either moved since `previous`.
pub fn worktree(shell: &dyn Shell, cwd: &str, previous: Option<&Worktree>) -> Option<Worktree> {
    let tree = crate::worktree::linked(shell, cwd).ok()??;
    let base_ref = crate::integration::base(shell, &tree.path).ok()?;
    let tips = git(shell, &tree.path, &["rev-parse", "HEAD", &base_ref])?;
    let mut tips = tips.lines();
    let (head, base) = (tips.next()?.to_owned(), tips.next()?.to_owned());
    if let Some(previous) = previous.filter(|p| {
        p.path == tree.path && p.branch == tree.branch && p.head == head && p.base == base
    }) {
        return Some(Worktree {
            dirty: !crate::loss::dirty_paths(shell, &tree.path, previous.landed)
                .ok()?
                .is_empty(),
            ..previous.clone()
        });
    }
    let ahead = git(
        shell,
        &tree.path,
        &["rev-list", "--count", &format!("{base}..{head}")],
    )?
    .parse()
    .ok()?;
    let landed = crate::loss::Evidence::default_only(&tree.path)
        .assess(shell, &format!("refs/heads/{}", tree.branch), None)
        .ok()?
        .landed();
    let dirty = !crate::loss::dirty_paths(shell, &tree.path, landed)
        .ok()?
        .is_empty();
    Some(Worktree {
        path: tree.path,
        branch: tree.branch,
        ahead,
        dirty,
        landed,
        head,
        base,
    })
}

/// Fill each agent's transcript and worktree facts, reusing `previous` where nothing changed.
pub fn observe(shell: &dyn Shell, probe: &mut Probe, previous: Option<&Probe>) {
    // Several agents can share one checkout. Observe it once per tick, but never cache its
    // dirty state across ticks: unchanged Git tips do not imply an unchanged index/worktree.
    let mut inputs = std::collections::BTreeMap::new();
    for agent in &mut probe.agent_panes {
        let old = previous.and_then(|p| {
            p.agent_panes
                .iter()
                .find(|a| a.pane == agent.pane && a.terminal_id == agent.terminal_id)
        });
        agent.session = transcript(agent, old);
        if let Some(cwd) = &agent.cwd {
            inputs
                .entry(cwd.as_str())
                .or_insert(old.and_then(|a| a.worktree.as_ref()));
        }
    }
    let mut trees: Vec<_> = inputs
        .into_iter()
        .map(|(cwd, old)| (cwd, old, None))
        .collect();
    let chunk = trees.len().div_ceil(crate::probe::cores() as usize).max(1);
    std::thread::scope(|scope| {
        for group in trees.chunks_mut(chunk) {
            scope.spawn(move || {
                for (cwd, old, observed) in group {
                    *observed = worktree(shell, cwd, *old);
                }
            });
        }
    });
    let trees: std::collections::BTreeMap<_, _> = trees
        .into_iter()
        .map(|(cwd, _, tree)| (cwd.to_owned(), tree))
        .collect();
    for agent in &mut probe.agent_panes {
        agent.worktree = agent
            .cwd
            .as_ref()
            .and_then(|cwd| trees.get(cwd))
            .cloned()
            .flatten();
    }
}

fn transcript(agent: &Agent, old: Option<&Agent>) -> Session {
    if let Some(old) = old.filter(|old| {
        old.transcript == agent.transcript && old.transcript_modified == agent.transcript_modified
    }) {
        return old.session.clone();
    }
    match (agent.agent.as_deref(), agent.transcript.as_deref()) {
        (Some(kind), Some(path)) => activity::tail(path)
            .map(|values| session(kind, &values))
            .unwrap_or_default(),
        _ => Session::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn newest_first(lines: &[Value]) -> Vec<Value> {
        lines.iter().rev().cloned().collect()
    }

    #[test]
    fn pi_reports_the_alias_and_the_answering_model() {
        let values = newest_first(&[
            json!({"type":"model_change","provider":"alias","modelId":"astra"}),
            json!({"type":"thinking_level_change","thinkingLevel":"high"}),
            json!({"type":"message","message":{"role":"assistant","provider":"alias","model":"astra","responseModel":"claude-opus-5-5","thinkingLevel":"high","content":[{"type":"toolCall","name":"edit","arguments":{"path":"src/api/dups.rs"}}]}}),
        ]);
        let session = session("pi", &values);
        assert_eq!(session.label().as_deref(), Some("astra→opus 5.5"));
        assert_eq!(session.thinking.as_deref(), Some("high"));
        assert_eq!(session.activity.as_deref(), Some("edit src/api/dups.rs"));
    }

    #[test]
    fn pi_collapses_an_alias_that_names_its_family() {
        let values = [
            json!({"type":"message","message":{"role":"assistant","model":"opus","responseModel":"claude-opus-5-5","content":"done"}}),
        ];
        assert_eq!(session("pi", &values).label().as_deref(), Some("opus 5.5"));
    }

    #[test]
    fn pi_shows_a_new_model_before_its_first_answer() {
        let values = newest_first(&[
            json!({"type":"message","message":{"role":"assistant","model":"opus","responseModel":"claude-opus-5-5","content":"done"}}),
            json!({"type":"model_change","provider":"alias","modelId":"sol"}),
        ]);
        assert_eq!(session("pi", &values).label().as_deref(), Some("sol"));
    }

    #[test]
    fn claude_skips_synthetic_messages() {
        let values = newest_first(&[
            json!({"type":"assistant","message":{"model":"claude-sonnet-5","content":[{"type":"text","text":"hi"}]}}),
            json!({"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"x"}]}}),
        ]);
        assert_eq!(
            session("claude", &values).label().as_deref(),
            Some("sonnet 5")
        );
    }

    #[test]
    fn codex_reads_model_and_effort_from_the_turn_context() {
        let values = newest_first(&[
            json!({"type":"turn_context","payload":{"model":"gpt-5.6-sol","effort":"high"}}),
            json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test -p auth\"}"}}),
        ]);
        let session = session("codex", &values);
        assert_eq!(session.label().as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(session.thinking.as_deref(), Some("high"));
        assert_eq!(session.activity.as_deref(), Some("cargo test -p auth"));
    }

    #[test]
    fn unknown_harnesses_and_empty_transcripts_have_no_label() {
        assert_eq!(session("other", &[]).label(), None);
        assert_eq!(pretty("claude-opus-5-20260101"), "opus 5");
    }

    fn linked(shell: &crate::shell::Fake, head: &str, base: &str) {
        shell
            .on(
                "git -C /t symbolic-ref --quiet refs/remotes/origin/HEAD",
                "refs/remotes/origin/main\n",
            )
            .on(
                "git -C /t rev-parse HEAD refs/remotes/origin/main",
                &format!("{head}\n{base}\n"),
            )
            .on("git -C /t status --porcelain --untracked-files=all -z", "");
    }

    #[test]
    fn shared_cwd_is_observed_once_each_tick() {
        let dir = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&dir).unwrap();
        let shell = crate::shell::Fake::new();
        let mut probe: Probe = serde_json::from_value(json!({"host":"local","label":"local","load1":0,"cores":4,
            "memory":{"used_bytes":0,"total_bytes":0},"disk_free_percent":50,"agents":{"total":2,"by_status":{}},
            "agent_panes":[{"pane":"a","state":"idle","cwd":dir},{"pane":"b","state":"working","cwd":dir}],
            "panes":[],"reservations":[]})).unwrap();
        observe(&shell, &mut probe, None);
        assert_eq!(shell.calls.lock().unwrap().len(), 1);
        let previous = probe.clone();
        observe(&shell, &mut probe, Some(&previous));
        assert_eq!(
            shell.calls.lock().unwrap().len(),
            2,
            "even a non-worktree is checked again next tick"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn worktree_counts_commits_only_when_a_tip_moves() {
        let dir = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&dir).unwrap();
        let cwd = dir.display().to_string();
        let shell = crate::shell::Fake::new();
        // `linked` needs a real directory; the fake answers for `/t` once it resolves.
        shell.on(
            &format!("git -C {cwd} rev-parse --path-format=absolute --git-common-dir --git-dir --show-toplevel --abbrev-ref HEAD"),
            "/r/.git\n/r/.git/worktrees/t\n/t\nfeat/x\n",
        ).on(&format!("git -C {cwd} rev-parse --path-format=absolute --git-common-dir"), "/r/.git\n");
        linked(&shell, "h1", "b1");
        shell
            .on("git -C /t rev-list --count b1..h1", "2\n")
            .on("git -C /t show-ref --verify --quiet refs/heads/feat/x", "")
            .on(
                "git -C /t rev-parse refs/heads/feat/x refs/remotes/origin/main",
                "h1\nb1\n",
            )
            .on_output(
                "git -C /t merge-base --is-ancestor h1 b1",
                crate::shell::Output {
                    status: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                },
            );
        let first = worktree(&shell, &cwd, None).unwrap();
        assert_eq!((first.ahead, first.landed, first.dirty), (2, true, false));
        let calls = shell.calls.lock().unwrap().len();
        shell.on(
            "git -C /t status --porcelain --untracked-files=all -z",
            " M a\0",
        );
        let second = worktree(&shell, &cwd, Some(&first)).unwrap();
        assert!(second.dirty);
        assert_eq!(second.ahead, 2);
        let counted = shell.calls.lock().unwrap()[calls..]
            .iter()
            .any(|c| c.contains("rev-list") || c.contains("show-ref"));
        assert!(
            !counted,
            "unchanged tips reuse the count and the landed check"
        );
        shell.on(
            "git -C /t status --porcelain --untracked-files=all -z",
            " D a\0D  b\0",
        );
        assert!(!worktree(&shell, &cwd, Some(&first)).unwrap().dirty);
        let unheld = Worktree {
            landed: false,
            ..first.clone()
        };
        assert!(worktree(&shell, &cwd, Some(&unheld)).unwrap().dirty);
        shell.on(
            "git -C /t status --porcelain --untracked-files=all -z",
            " D a\0 M b\0?? c\0",
        );
        assert!(worktree(&shell, &cwd, Some(&first)).unwrap().dirty);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
