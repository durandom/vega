//! What a watch points at: a PR (`o/r#42`) or a branch (`o/r@feat`) on a
//! GitHub or Forgejo host, or a GitHub Actions run (`o/r/run/123`) or the
//! next `workflow_dispatch` run of a workflow (`o/r/workflow/deploy.yml@main`).

use serde::{Deserialize, Serialize};

use crate::forge::{Forge, Probe};

pub const GITHUB: &str = "github.com";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub host: String,
    /// Absent in files written before Forgejo support: GitHub.
    #[serde(default, skip_serializing_if = "Forge::is_github")]
    pub forge: Forge,
    /// `owner/repo`
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// A GitHub Actions run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<u64>,
    /// A workflow file (or id) whose `workflow_dispatch` run vegad resolves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// The branch, tag or sha the workflow was dispatched on.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
}

impl Target {
    pub fn pr(host: &str, repo: &str, n: u64) -> Self {
        Target {
            host: host.into(),
            repo: repo.into(),
            pr: Some(n),
            ..Target::default()
        }
    }

    pub fn branch(host: &str, repo: &str, b: &str) -> Self {
        Target {
            host: host.into(),
            repo: repo.into(),
            branch: Some(b.into()),
            ..Target::default()
        }
    }

    pub fn run(host: &str, repo: &str, id: u64) -> Self {
        Target {
            host: host.into(),
            repo: repo.into(),
            run: Some(id),
            ..Target::default()
        }
    }

    /// The `workflow_dispatch` run of `file` on `git_ref` (any ref if `None`).
    pub fn workflow(host: &str, repo: &str, file: &str, git_ref: Option<&str>) -> Self {
        Target {
            host: host.into(),
            repo: repo.into(),
            workflow: Some(file.into()),
            git_ref: git_ref.map(String::from),
            ..Target::default()
        }
    }

    /// A run or workflow target: GitHub Actions only, ends with the run.
    pub fn is_run(&self) -> bool {
        self.run.is_some() || self.workflow.is_some()
    }

    pub fn on(mut self, forge: Forge) -> Self {
        self.forge = forge;
        self
    }

    /// The forge of the host from `probe` (or its cache, see [`crate::forge::resolve`]).
    pub fn resolved(self, probe: &mut Probe) -> Result<Target, String> {
        let f = crate::forge::resolve(&self.host, probe)?;
        Ok(self.on(f))
    }

    /// `o/r#42`, `o/r@feat`; prefixed with the host when it is not github.com.
    /// The forge is not in the label: it follows from the host.
    pub fn label(&self) -> String {
        let host = if self.host == GITHUB {
            String::new()
        } else {
            format!("{}/", self.host)
        };
        let repo = &self.repo;
        if let Some(n) = self.run {
            return format!("{host}{repo}/run/{n}");
        }
        if let Some(w) = &self.workflow {
            return match &self.git_ref {
                Some(r) => format!("{host}{repo}/workflow/{w}@{r}"),
                None => format!("{host}{repo}/workflow/{w}"),
            };
        }
        match (&self.pr, &self.branch) {
            (Some(n), _) => format!("{host}{repo}#{n}"),
            (None, Some(b)) => format!("{host}{repo}@{b}"),
            (None, None) => format!("{host}{repo}"),
        }
    }

    /// The label for humans in a mail: `PR #42 (o/r)`, `run 123 (o/r)`,
    /// `deploy.yml run (o/r@main)` or `o/r@feat`.
    pub fn title(&self) -> String {
        self.title_of(None)
    }

    /// [`Target::title`] with the run id a workflow target resolved to:
    /// `deploy.yml run 123 (o/r@main)`.
    pub fn title_of(&self, resolved: Option<u64>) -> String {
        if let Some(n) = self.run {
            return format!("run {n} ({})", self.repo);
        }
        if let Some(w) = &self.workflow {
            let run = resolved.map_or("run".to_string(), |n| format!("run {n}"));
            return match &self.git_ref {
                Some(r) => format!("{w} {run} ({}@{r})", self.repo),
                None => format!("{w} {run} ({})", self.repo),
            };
        }
        match self.pr {
            Some(n) => format!("PR #{n} ({})", self.repo),
            None => self.label(),
        }
    }

    /// A file-name-safe key, unique per label.
    pub fn key(&self) -> String {
        self.label()
            .chars()
            .map(|c| match c {
                '#' => '+',
                '@' => '=',
                c if c.is_ascii_alphanumeric() || "-_.+=".contains(c) => c,
                _ => '_',
            })
            .collect()
    }

    /// Parses a label as [`Target::label`] prints it; the forge of its host
    /// comes from `probe` (or the cache).
    pub fn parse_label(s: &str, probe: &mut Probe) -> Result<Target, String> {
        Target::parse_host_label(s)
            .ok_or_else(|| {
                format!("not a target: {s} (owner/repo#42, owner/repo@branch, owner/repo/run/123)")
            })?
            .resolved(probe)
    }

    /// [`Target::parse_label`] before the forge is known.
    fn parse_host_label(s: &str) -> Option<Target> {
        // Owner names have no dot, hosts do.
        let (host, rest) = match s.split_once('/') {
            Some((h, r)) if h.contains('.') => (h, r),
            _ => (GITHUB, s),
        };
        let (owner, rest) = rest.split_once('/')?;
        // A repo name has none of `#@/`: the first one says the kind (a branch
        // after `@` may hold `/`).
        let at = rest.find(['#', '@', '/'])?;
        let repo = format!("{owner}/{}", &rest[..at]);
        let tail = &rest[at + 1..];
        match &rest[at..=at] {
            "#" => Some(Target::pr(host, &repo, tail.parse().ok()?)),
            "@" => (!tail.is_empty()).then(|| Target::branch(host, &repo, tail)),
            _ => {
                if let Some(n) = tail.strip_prefix("run/") {
                    return Some(Target::run(host, &repo, n.parse().ok()?));
                }
                let w = tail.strip_prefix("workflow/")?;
                let (file, git_ref) = match w.split_once('@') {
                    Some((f, r)) => (f, Some(r).filter(|r| !r.is_empty())),
                    None => (w, None),
                };
                (!file.is_empty()).then(|| Target::workflow(host, &repo, file, git_ref))
            }
        }
    }

    /// A user's argument: `42`, `#42`, a PR URL (GitHub `/pull/N`, Forgejo
    /// `/pulls/N`), a run URL (`/actions/runs/N`, as `gh workflow run` prints
    /// it), a label, or a branch name of `default` (host, repo). The
    /// forge of the host comes from `probe` (or the cache).
    pub fn from_arg(
        arg: &str,
        default: Option<(&str, &str)>,
        probe: &mut Probe,
    ) -> Result<Target, String> {
        Target::from_arg_on_host(arg, default)?.resolved(probe)
    }

    /// [`Target::from_arg`] before the forge is known.
    fn from_arg_on_host(arg: &str, default: Option<(&str, &str)>) -> Result<Target, String> {
        let arg = arg.trim();
        if let Some(url) = arg.strip_prefix("https://") {
            let parts: Vec<&str> = url.trim_end_matches('/').split('/').collect();
            return match parts.as_slice() {
                [host, o, r, "pull" | "pulls", n, ..] => n
                    .parse()
                    .map(|n| Target::pr(host, &format!("{o}/{r}"), n))
                    .map_err(|_| format!("no PR number in {arg}")),
                [host, o, r, "actions", "runs", n, ..] => n
                    .parse()
                    .map(|n| Target::run(host, &format!("{o}/{r}"), n))
                    .map_err(|_| format!("no run id in {arg}")),
                _ => Err(format!("not a PR or run URL: {arg}")),
            };
        }
        if arg.contains('#')
            || (arg.contains('@') && arg.contains('/'))
            || arg.contains("/run/")
            || arg.contains("/workflow/")
        {
            if let Some(t) = Target::parse_host_label(arg) {
                return Ok(t);
            }
        }
        let (host, repo) = default.ok_or_else(|| {
            format!("no repo for {arg}: run it in a GitHub checkout or pass -R owner/repo")
        })?;
        match arg.trim_start_matches('#').parse::<u64>() {
            Ok(n) => Ok(Target::pr(host, repo, n)),
            Err(_) if arg.is_empty() => Err("empty target".into()),
            Err(_) => Ok(Target::branch(host, repo, arg)),
        }
    }
}

/// (host, `owner/repo`) of a git remote URL: `git@github.com:o/r.git`,
/// `https://github.com/o/r`, `ssh://git@github.com/o/r.git`,
/// `ssh://git@host:2222/o/r.git` (the port is dropped).
pub fn parse_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://"))
        .map(|r| r.rsplit_once('@').map_or(r, |(_, h)| h).to_string())
        .or_else(|| {
            // scp form: user@host:owner/repo
            let (_, hp) = url.split_once('@')?;
            Some(hp.replacen(':', "/", 1))
        })?;
    let mut parts = rest.split('/');
    let host = parts.next()?.split(':').next()?.to_string();
    let owner = parts.next()?;
    let repo = parts.next()?;
    (parts.next().is_none() && !owner.is_empty() && !repo.is_empty())
        .then(|| (host, format!("{owner}/{repo}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORGEJO_HOST: &str = "git.b4mad.industries";

    /// Forgejo on [`FORGEJO_HOST`], GitHub elsewhere; github.com is never probed.
    fn probe(host: &str) -> Result<(u16, String), String> {
        assert_ne!(host, GITHUB, "probed github.com");
        Ok(match host {
            FORGEJO_HOST => (200, r#"{"version":"16.0.5+gitea-1.22.0"}"#.into()),
            _ => (404, String::new()),
        })
    }

    fn with_state_dir<R>(f: impl FnOnce() -> R) -> R {
        // Tests in this binary that resolve forges share this lock: the cache
        // lives under $XDG_STATE_HOME.
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_STATE_HOME", tmp.path());
        f()
    }

    #[test]
    fn labels_round_trip() {
        with_state_dir(|| {
            for t in [
                Target::pr(GITHUB, "o/r", 42),
                Target::branch(GITHUB, "o/r", "feat/x"),
                Target::pr("ghe.corp.com", "o/r", 7),
                Target::branch("ghe.corp.com", "o/r", "a@b"),
                Target::pr(FORGEJO_HOST, "o/r", 5).on(Forge::Forgejo),
                Target::branch(FORGEJO_HOST, "o/r", "feat/x").on(Forge::Forgejo),
                Target::run(GITHUB, "o/r", 38721967731),
                Target::workflow(GITHUB, "o/r", "deploy.yml", Some("main")),
                Target::workflow(GITHUB, "o/r", "deploy.yml", Some("release/1.x")),
                Target::workflow(GITHUB, "o/r", "161335", None),
            ] {
                assert_eq!(
                    Target::parse_label(&t.label(), &mut probe),
                    Ok(t.clone()),
                    "{}",
                    t.label()
                );
            }
        });
        assert_eq!(Target::pr(GITHUB, "o/r", 42).key(), "o_r+42");
        assert_eq!(Target::branch(GITHUB, "o/r", "feat/x").key(), "o_r=feat_x");
        let fj = Target::pr(FORGEJO_HOST, "o/r", 5).on(Forge::Forgejo);
        assert_eq!(fj.label(), "git.b4mad.industries/o/r#5");
        assert_eq!(fj.key(), "git.b4mad.industries_o_r+5");
        let run = Target::run(GITHUB, "o/r", 7);
        assert_eq!(
            (run.label(), run.key()),
            ("o/r/run/7".into(), "o_r_run_7".into())
        );
        let w = Target::workflow(GITHUB, "o/r", "deploy.yml", Some("main"));
        assert_eq!(w.label(), "o/r/workflow/deploy.yml@main");
        assert_eq!(w.title_of(Some(9)), "deploy.yml run 9 (o/r@main)");
    }

    #[test]
    fn run_urls_and_labels_are_runs_but_branches_stay_branches() {
        with_state_dir(|| {
            let p = &mut probe;
            assert_eq!(
                Target::from_arg("https://github.com/o/r/actions/runs/38721967731", None, p),
                Ok(Target::run(GITHUB, "o/r", 38721967731))
            );
            assert_eq!(
                Target::from_arg("o/r/run/5", None, p),
                Ok(Target::run(GITHUB, "o/r", 5))
            );
            let d = Some((GITHUB, "o/r"));
            assert_eq!(
                Target::from_arg("feat/run/x", d, p),
                Ok(Target::branch(GITHUB, "o/r", "feat/run/x"))
            );
            assert!(Target::run(GITHUB, "o/r", 1).is_run());
            assert!(!Target::pr(GITHUB, "o/r", 1).is_run());
        });
    }

    #[test]
    fn args_resolve_against_the_default_repo() {
        with_state_dir(|| {
            let d = Some((GITHUB, "o/r"));
            let p = &mut probe;
            assert_eq!(
                Target::from_arg("42", d, p),
                Ok(Target::pr(GITHUB, "o/r", 42))
            );
            assert_eq!(
                Target::from_arg("#42", d, p),
                Ok(Target::pr(GITHUB, "o/r", 42))
            );
            assert_eq!(
                Target::from_arg("feat/x", d, p),
                Ok(Target::branch(GITHUB, "o/r", "feat/x"))
            );
            assert_eq!(
                Target::from_arg("a/b#3", None, p),
                Ok(Target::pr(GITHUB, "a/b", 3))
            );
            assert_eq!(
                Target::from_arg("https://github.com/a/b/pull/9/checks", None, p),
                Ok(Target::pr(GITHUB, "a/b", 9))
            );
            assert!(Target::from_arg("42", None, p).is_err());
        });
    }

    #[test]
    fn forgejo_args_resolve() {
        with_state_dir(|| {
            let p = &mut probe;
            let want = Target::pr(FORGEJO_HOST, "o/r", 9).on(Forge::Forgejo);
            assert_eq!(
                Target::from_arg("https://git.b4mad.industries/o/r/pulls/9", None, p),
                Ok(want.clone())
            );
            assert_eq!(Target::from_arg(&want.label(), None, p), Ok(want.clone()));
            let d = Some((FORGEJO_HOST, "o/r"));
            assert_eq!(Target::from_arg("9", d, p), Ok(want));
            assert_eq!(
                Target::from_arg("feat", d, p),
                Ok(Target::branch(FORGEJO_HOST, "o/r", "feat").on(Forge::Forgejo))
            );
            // The answer is cached: no second probe.
            let mut never = |_: &str| -> Result<(u16, String), String> { panic!("probed twice") };
            assert_eq!(
                Target::from_arg("9", d, &mut never).map(|t| t.forge),
                Ok(Forge::Forgejo)
            );
        });
    }

    #[test]
    fn a_failed_probe_is_an_error_and_not_cached() {
        with_state_dir(|| {
            let mut down = |_: &str| -> Result<(u16, String), String> { Err("timeout".into()) };
            assert!(Target::from_arg("x.org/o/r#1", None, &mut down).is_err());
            assert_eq!(
                Target::from_arg("x.org/o/r#1", None, &mut probe).map(|t| t.forge),
                Ok(Forge::GitHub)
            );
        });
    }

    #[test]
    fn only_a_sure_answer_is_cached() {
        with_state_dir(|| {
            // A login wall (401) is a guess: GitHub now, probed again next time.
            let mut walled =
                |_: &str| -> Result<(u16, String), String> { Ok((401, String::new())) };
            assert_eq!(
                Target::from_arg("x.org/o/r#1", None, &mut walled).map(|t| t.forge),
                Ok(Forge::GitHub)
            );
            let mut forgejo = |_: &str| -> Result<(u16, String), String> {
                Ok((200, r#"{"version":"11.0.0"}"#.into()))
            };
            assert_eq!(
                Target::from_arg("x.org/o/r#1", None, &mut forgejo).map(|t| t.forge),
                Ok(Forge::Forgejo)
            );
        });
    }

    #[test]
    fn github_targets_serialize_without_forge() {
        let gh = serde_json::to_value(Target::pr(GITHUB, "o/r", 1)).unwrap();
        assert!(gh.get("forge").is_none());
        let fj = Target::pr(FORGEJO_HOST, "o/r", 1).on(Forge::Forgejo);
        let v = serde_json::to_value(&fj).unwrap();
        assert_eq!(v["forge"], "forgejo");
        assert_eq!(serde_json::from_value::<Target>(v).unwrap(), fj);
    }

    #[test]
    fn remotes_parse() {
        let want = Some((GITHUB.to_string(), "o/r".to_string()));
        assert_eq!(parse_remote("git@github.com:o/r.git"), want);
        assert_eq!(parse_remote("https://github.com/o/r"), want);
        assert_eq!(parse_remote("https://github.com/o/r.git/"), want);
        assert_eq!(parse_remote("ssh://git@github.com/o/r.git"), want);
        assert_eq!(
            parse_remote("ssh://git@git.b4mad.industries:2222/o/r.git"),
            Some((FORGEJO_HOST.to_string(), "o/r".to_string()))
        );
        assert_eq!(parse_remote("/local/path"), None);
    }
}
