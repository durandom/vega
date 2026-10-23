//! Fixture JSONL per agent kind: synthetic content in the real formats.

use std::fs;
use std::path::{Path, PathBuf};

use vega_transcript::{locate, parse, Entry, Kind, LocateError, Role};

fn fixture(kind: Kind) -> Vec<Entry> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{}.jsonl", kind.as_str()));
    parse(kind, &fs::read_to_string(path).unwrap())
}

fn roles_texts(entries: &[Entry]) -> Vec<(&str, &str)> {
    entries
        .iter()
        .map(|e| (e.role.as_str(), e.text.as_str()))
        .collect()
}

#[test]
fn claude_drops_meta_thinking_results_and_sidechains() {
    let e = fixture(Kind::Claude);
    assert_eq!(
        roles_texts(&e),
        [
            ("user", "Run the tests and report."),
            ("tool", "Bash: mise run check"),
            ("assistant", "All tests pass.\n\nGate: exit 0."),
            ("user", "Now commit."),
            ("error", "API Error: 429 rate limited"),
        ]
    );
    assert_eq!(
        e[0].time,
        vega_transcript::parse_rfc3339("2026-10-10T07:54:01Z")
    );
}

#[test]
fn codex_drops_injected_context_and_reasoning() {
    assert_eq!(
        roles_texts(&fixture(Kind::Codex)),
        [
            ("user", "List the files."),
            ("assistant", "Listing them."),
            ("tool", "exec: ls -la"),
            ("tool", "shell: git status"),
            ("assistant", "Two files: a and b."),
            ("error", "stream disconnected: 429 Too Many Requests"),
        ]
    );
}

#[test]
fn pi_keeps_text_tool_calls_and_errors() {
    assert_eq!(
        roles_texts(&fixture(Kind::Pi)),
        [
            ("user", "Read the README."),
            ("tool", "read: README.md"),
            ("assistant", "It is about repo."),
            ("error", "terminated"),
        ]
    );
}

#[test]
fn agy_keeps_planner_text_and_tool_calls() {
    let e = fixture(Kind::Agy);
    assert_eq!(
        roles_texts(&e),
        [
            ("user", "What is in src?"),
            ("tool", "list_dir: src"),
            ("assistant", "src holds lib.rs."),
        ]
    );
    assert!(e.iter().all(|x| x.time.is_some()));
    assert_eq!(e[0].role, Role::User);
}

/// A scratch `$HOME` per test.
fn home(name: &str) -> PathBuf {
    let h = std::env::temp_dir().join(format!("vega-transcript-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&h);
    fs::create_dir_all(&h).unwrap();
    h
}

fn touch(p: &Path, body: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

#[test]
fn locate_claude_by_id_by_moved_cwd_and_by_cwd() {
    let h = home("claude");
    let f = h.join(".claude/projects/-w-repo-x/abc.jsonl");
    touch(&f, "");
    assert_eq!(
        locate(Kind::Claude, Some("abc"), "/w/repo.x", &h),
        Ok(f.clone())
    );
    // The pane's cwd moved: the id still finds it.
    assert_eq!(
        locate(Kind::Claude, Some("abc"), "/elsewhere", &h),
        Ok(f.clone())
    );
    assert_eq!(locate(Kind::Claude, None, "/w/repo.x", &h), Ok(f.clone()));
    touch(&h.join(".claude/projects/-w-repo-x/def.jsonl"), "");
    match locate(Kind::Claude, None, "/w/repo.x", &h) {
        Err(LocateError::Ambiguous { candidates }) => assert_eq!(candidates.len(), 2),
        r => panic!("{r:?}"),
    }
    assert!(matches!(
        locate(Kind::Claude, Some("nope"), "/w/repo.x", &h),
        Err(LocateError::NotFound { .. })
    ));
}

#[test]
fn locate_codex_by_id_and_by_session_meta_cwd() {
    let h = home("codex");
    let f = h.join(".codex/sessions/2026/10/10/rollout-2026-10-10T09-00-00-c1.jsonl");
    touch(
        &f,
        "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/work/repo\"}}\n",
    );
    touch(
        &h.join(".codex/sessions/2026/10/09/rollout-2026-10-09T09-00-00-c0.jsonl"),
        "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/other\"}}\n",
    );
    assert_eq!(locate(Kind::Codex, Some("c1"), "/x", &h), Ok(f.clone()));
    assert_eq!(locate(Kind::Codex, None, "/work/repo", &h), Ok(f));
    assert!(matches!(
        locate(Kind::Codex, None, "/none", &h),
        Err(LocateError::NotFound { .. })
    ));
}

#[test]
fn locate_pi_by_path_and_id_and_agy_by_id() {
    let h = home("pi");
    let f = h.join(".pi/agent/sessions/--work-repo--/2026-10-10T07-54-00-000Z_p1.jsonl");
    touch(&f, "");
    let path = f.display().to_string();
    assert_eq!(locate(Kind::Pi, Some(&path), "/x", &h), Ok(f.clone()));
    assert_eq!(
        locate(Kind::Pi, Some("p1"), "/work/repo", &h),
        Ok(f.clone())
    );
    assert_eq!(locate(Kind::Pi, None, "/work/repo", &h), Ok(f));
    let a = h.join(".gemini/antigravity-cli/brain/g1/.system_generated/logs/transcript.jsonl");
    touch(&a, "");
    assert_eq!(locate(Kind::Agy, Some("g1"), "/x", &h), Ok(a));
    assert!(matches!(
        locate(Kind::Agy, None, "/x", &h),
        Err(LocateError::NotFound { .. })
    ));
}
