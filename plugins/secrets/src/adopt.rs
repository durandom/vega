//! `scan` and `adopt`: find secrets in a checkout's `.envrc` and `.env*`
//! files (names only) and move them into vega secrets; direnv then loads them
//! with `use vega <tool>` (docs/design/secrets.md, Projects).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use vega_secrets::{Entry, Error, Here, Manifest, Secret, Store};

/// How a file holds a variable, judged without showing the value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Class {
    /// A plain value in the file.
    Literal,
    /// Read from 1Password (`op read`, `op://`).
    Op,
    /// Another variable or a command (`$X`, `$(...)`).
    Ref,
    Empty,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::Literal => "literal",
            Class::Op => "op",
            Class::Ref => "ref",
            Class::Empty => "empty",
        }
    }
}

/// One secret-like assignment in an env file.
#[derive(Debug, Clone, PartialEq)]
pub struct Var {
    pub file: PathBuf,
    pub name: String,
    pub class: Class,
}

const SECRET_WORDS: [&str; 8] = [
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "PAT",
    "CREDENTIAL",
    "CREDENTIALS",
];
const NOT_SECRET_LAST: [&str; 9] = [
    "PATH", "FILE", "DIR", "URL", "URI", "ENABLED", "ID", "MINUTES", "DAYS",
];
const SKIP_DIRS: [&str; 6] = [
    ".git",
    "node_modules",
    "target",
    ".venv",
    "vendor",
    ".direnv",
];

/// Whether a variable name looks like it holds a secret: one of its `_`
/// segments is KEY, TOKEN, SECRET, ... and the last is not PATH, FILE, URL, ...
pub fn is_secret_name(name: &str) -> bool {
    let parts: Vec<&str> = name.split('_').filter(|p| !p.is_empty()).collect();
    let Some(last) = parts.last() else {
        return false;
    };
    let upper = |p: &&str| p.to_ascii_uppercase();
    !NOT_SECRET_LAST.contains(&upper(last).as_str())
        && parts
            .iter()
            .any(|p| SECRET_WORDS.contains(&upper(p).as_str()))
}

/// `NAME` and the raw value of an assignment line (`[export ]NAME=value`).
pub fn parse_line(line: &str) -> Option<(&str, &str)> {
    let l = line.trim_start();
    let l = l.strip_prefix("export ").map(str::trim_start).unwrap_or(l);
    let end = l
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(l.len());
    let (name, rest) = l.split_at(end);
    let value = rest.trim_start().strip_prefix('=')?;
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some((name, value.trim()))
}

pub fn classify(raw: &str) -> Class {
    let v = unquote(raw);
    if v.is_empty() {
        Class::Empty
    } else if raw.contains("op read") || raw.contains("op://") {
        Class::Op
    } else if raw.contains('$') || raw.contains('`') {
        Class::Ref
    } else {
        Class::Literal
    }
}

/// The value of a literal: quotes removed, an unquoted ` #` comment cut.
pub fn unquote(raw: &str) -> String {
    let r = raw.trim();
    for q in ['"', '\''] {
        if let Some(inner) = r.strip_prefix(q) {
            if let Some(end) = inner.find(q) {
                return inner[..end].to_string();
            }
        }
    }
    match r.find(" #") {
        Some(i) => r[..i].trim_end().to_string(),
        None => r.to_string(),
    }
}

/// The `op://...` reference in a raw value.
pub fn op_ref(raw: &str) -> Option<String> {
    let i = raw.find("op://")?;
    let r: String = raw[i..]
        .chars()
        .take_while(|c| !matches!(c, '"' | '\'' | ')' | '`') && !c.is_whitespace())
        .collect();
    Some(r)
}

fn is_env_file(name: &str) -> bool {
    let skip = [".example", ".sample", ".template", ".dist"];
    (name == ".envrc" || name == ".env" || name.starts_with(".env.") || name.ends_with(".env"))
        && !skip.iter().any(|s| name.ends_with(s))
}

/// `.envrc` and `.env*` files under `dir`, at most `depth` levels down.
pub fn env_files(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return out;
    };
    let mut items: Vec<_> = rd.flatten().collect();
    items.sort_by_key(|e| e.file_name());
    for e in items {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && depth > 0 && !SKIP_DIRS.contains(&name.as_str()) {
            out.extend(env_files(&e.path(), depth - 1));
        } else if ft.is_file() && is_env_file(&name) {
            out.push(e.path());
        }
    }
    out
}

/// The secret-like variables of one file's text, first assignment of each.
pub fn scan_text(file: &Path, text: &str) -> Vec<Var> {
    let mut v: Vec<Var> = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        let Some((name, raw)) = parse_line(line) else {
            continue;
        };
        if is_secret_name(name) && !v.iter().any(|x| x.name == name) {
            v.push(Var {
                file: file.to_path_buf(),
                name: name.to_string(),
                class: classify(raw),
            });
        }
    }
    v
}

pub fn has_use_vega(text: &str, tool: &str) -> bool {
    let want = format!("use vega {tool}");
    text.lines().any(|l| l.trim() == want)
}

/// The text without the assignments of `names`; with `use_tool`, also with a
/// `use vega <tool>` line (after a leading shebang) unless it has one, followed
/// by the `extra` lines it lacks (aliases, see [`Adopted::alias`]).
pub fn rewrite(text: &str, names: &[String], use_tool: Option<&str>, extra: &[String]) -> String {
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|l| {
            l.trim_start().starts_with('#')
                || !parse_line(l).is_some_and(|(n, _)| names.iter().any(|x| x == n))
        })
        .collect();
    let use_line;
    if let Some(tool) = use_tool {
        use_line = format!("use vega {tool}");
        let at = match lines.iter().position(|l| l.trim() == use_line) {
            Some(i) => i + 1,
            None => {
                let at = usize::from(lines.first().is_some_and(|l| l.starts_with("#!")));
                lines.insert(at, &use_line);
                at + 1
            }
        };
        let missing: Vec<&str> = extra
            .iter()
            .map(String::as_str)
            .filter(|x| !lines.iter().any(|l| l.trim() == *x))
            .collect();
        for (i, x) in missing.into_iter().enumerate() {
            lines.insert(at + i, x);
        }
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// A secret name from an env var: `OPENAI_API_KEY` -> `openai`.
pub fn derive_name(env: &str) -> String {
    let up = env.to_ascii_uppercase();
    let suffixes = [
        "_API_KEY",
        "_API_TOKEN",
        "_ACCESS_TOKEN",
        "_AUTH_TOKEN",
        "_TOKEN",
        "_KEY",
        "_SECRET",
        "_PASSWORD",
    ];
    let stem = suffixes
        .iter()
        .find_map(|s| up.strip_suffix(s).filter(|r| !r.is_empty()))
        .unwrap_or(&up);
    let mut name = String::new();
    for c in stem.to_ascii_lowercase().chars() {
        let c = if c.is_ascii_alphanumeric() { c } else { '-' };
        if !(c == '-' && (name.is_empty() || name.ends_with('-'))) {
            name.push(c);
        }
    }
    name.trim_end_matches('-').to_string()
}

/// The 1Password side of `adopt`; a trait so tests run without `op`.
pub trait Vault {
    /// The ref of an item's credential field: `op://<vault>/<title>/credential`.
    fn reference(&self, title: &str) -> String;
    fn exists(&self, title: &str) -> Result<bool, Error>;
    fn read(&self, reference: &str) -> Result<Secret, Error>;
    /// Creates an API Credential item tagged `vega`; the value goes over stdin.
    fn create(&self, title: &str, notes: &str, value: &Secret) -> Result<(), Error>;
}

pub struct Op {
    pub vault: String,
}

fn op_missing() -> Error {
    Error::new(
        "op_missing",
        "op is not installed",
        "brew install 1password-cli",
    )
}

impl Vault for Op {
    fn reference(&self, title: &str) -> String {
        format!("op://{}/{title}/credential", self.vault)
    }

    fn exists(&self, title: &str) -> Result<bool, Error> {
        let s = Command::new("op")
            .args([
                "item",
                "get",
                title,
                "--vault",
                &self.vault,
                "--format",
                "json",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| op_missing())?;
        Ok(s.success())
    }

    fn read(&self, reference: &str) -> Result<Secret, Error> {
        let out = Command::new("op")
            .args(["read", "--no-newline", reference])
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|_| op_missing())?;
        if !out.status.success() {
            return Err(Error::new(
                "op_failed",
                format!("op read {reference} failed (see stderr)"),
                "op signin; check the reference with op item get",
            ));
        }
        String::from_utf8(out.stdout)
            .map(Secret::new)
            .map_err(|_| Error::new("not_utf8", "the op value is not UTF-8", "check the item"))
    }

    fn create(&self, title: &str, notes: &str, value: &Secret) -> Result<(), Error> {
        use std::io::Write;
        let json = format!(
            r#"{{"title":{},"category":"API_CREDENTIAL","tags":["vega"],"fields":[{{"id":"credential","type":"CONCEALED","label":"credential","value":{}}},{{"id":"notesPlain","type":"STRING","purpose":"NOTES","label":"notesPlain","value":{}}}]}}"#,
            json_str(title),
            json_str(value.expose()),
            json_str(notes)
        );
        let json = Secret::new(json);
        let mut child = Command::new("op")
            .args(["item", "create", "--vault", &self.vault, "--format", "json"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|_| op_missing())?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(json.expose().as_bytes());
        }
        let ok = child.wait().is_ok_and(|s| s.success());
        if ok {
            Ok(())
        } else {
            Err(Error::new(
                "op_failed",
                format!("op item create {title} failed (see stderr)"),
                "op signin, then retry",
            ))
        }
    }
}

/// A JSON string literal.
fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What `adopt_value` did with one variable.
#[derive(Debug, PartialEq)]
pub struct Adopted {
    pub var: String,
    pub secret: String,
    /// `existing` (same value already in vega), `alias` (same value already in
    /// vega under another env name), `op` (same value already in 1Password) or
    /// `new` (1Password item created).
    pub state: &'static str,
    pub tools: Vec<String>,
    /// The secret's env name when it differs from `var`.
    pub env: Option<String>,
}

impl Adopted {
    /// The `.envrc` line that gives `var` the secret's value, for `alias`.
    pub fn alias(&self) -> Option<String> {
        self.env
            .as_ref()
            .map(|env| format!("export {}=\"${env}\"", self.var))
    }
}

/// Puts one value into vega secrets for `tool`. A secret with the same env
/// and the same value gets `tool` added to its `for`; otherwise a new secret
/// `<name>` (or `<name>-<tool>` when `<name>` holds another value) is created
/// from the 1Password item `<name>-api` if it holds the same value, else from
/// `op_ref` (the file read the value from 1Password), else from a new item.
#[allow(clippy::too_many_arguments)]
pub fn adopt_value(
    store: &Store,
    m: &mut Manifest,
    vault: &dyn Vault,
    var: &str,
    tool: &str,
    value: &Secret,
    name: Option<&str>,
    op_ref: Option<String>,
    origin: &str,
) -> Result<Adopted, Error> {
    // Same value already in vega: never a second copy. Same env name first, so
    // an alias is only used when no secret holds it under `var` itself.
    let mut known = m.entries();
    known.sort_by_key(|e| e.env != var);
    for mut e in known {
        if store.here(m, &e) != Here::Ok || store.decrypt(&e.name)?.expose() != value.expose() {
            continue;
        }
        if !e.tools.iter().any(|t| t == tool) {
            e.tools.push(tool.to_string());
            m.set_entry(&e);
        }
        let alias = e.env != var;
        return Ok(Adopted {
            var: var.into(),
            secret: e.name.clone(),
            state: if alias { "alias" } else { "existing" },
            tools: e.tools,
            env: alias.then(|| e.env.clone()),
        });
    }
    let base = name.map(str::to_owned).unwrap_or_else(|| derive_name(var));
    let candidates = if name.is_some() {
        vec![base.clone()]
    } else {
        vec![base.clone(), format!("{base}-{tool}")]
    };
    for cand in candidates {
        if !vega_secrets::valid_name(&cand) || m.entry(&cand).is_some() {
            continue;
        }
        let title = format!("{cand}-api");
        let (reference, state) = if vault.exists(&title)? {
            let r = vault.reference(&title);
            if vault.read(&r)?.expose() != value.expose() {
                continue;
            }
            (r, "op")
        } else if let Some(r) = &op_ref {
            (r.clone(), "op")
        } else {
            vault.create(&title, &format!("env: {var}\nfrom: {origin}"), value)?;
            (vault.reference(&title), "new")
        };
        let mut e = Entry {
            name: cand.clone(),
            env: var.into(),
            tools: vec![tool.into()],
            op: Some(reference),
            ..Entry::default()
        };
        e.encrypted_to = store.encrypt(m, &e, value)?;
        e.rotated = Some(vega_secrets::today());
        m.set_entry(&e);
        return Ok(Adopted {
            var: var.into(),
            secret: cand,
            state,
            tools: e.tools,
            env: None,
        });
    }
    Err(Error::new(
        "name_taken",
        format!("{base} (and {base}-{tool}) already hold another value"),
        format!("pass --name <name> for {var}"),
    ))
}

/// An exclusive lock on the manifest for the life of the value, so UACs that
/// adopt at the same time do not overwrite each other's entries.
pub struct Lock(PathBuf);

impl Lock {
    pub fn acquire(store: &Store) -> Result<Self, Error> {
        let path = store.config.join("secrets.lock");
        let _ = fs::create_dir_all(&store.config);
        for _ in 0..600 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Self(path)),
                Err(_) => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .is_some_and(|age| age > Duration::from_secs(600));
                    if stale {
                        let _ = fs::remove_file(&path);
                    } else {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        }
        Err(Error::new(
            "locked",
            format!("{} is held by another adopt", path.display()),
            "retry in a minute; remove the file if no adopt is running",
        ))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Writes `text` over `path`, keeping its permissions.
pub fn write_keep_mode(path: &Path, text: &str) -> Result<(), Error> {
    let io = |e: std::io::Error| {
        Error::new(
            "write_failed",
            format!("{}: {e}", path.display()),
            "check the file's permissions",
        )
    };
    let mode = fs::metadata(path)
        .map(|m| m.permissions().mode())
        .unwrap_or(0o600);
    let tmp = path.with_extension("vega-tmp");
    fs::write(&tmp, text).map_err(io)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)).map_err(io)?;
    fs::rename(&tmp, path).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn secret_names() {
        for n in [
            "OPENAI_API_KEY",
            "FAL_KEY",
            "PMV_AUTH__API_KEY",
            "GH_PAT",
            "DB_PASSWORD",
            "SLACK_MCP_XOXC_TOKEN",
        ] {
            assert!(is_secret_name(n), "{n}");
        }
        for n in [
            "OAUTHLIB_INSECURE_TRANSPORT",
            "GOOGLE_CLIENT_SECRET_PATH",
            "PATH",
            "GDRIVE_SERVICE_ACCOUNT_PATH",
            "KEYBOARD",
            "",
        ] {
            assert!(!is_secret_name(n), "{n}");
        }
    }

    #[test]
    fn lines_and_classes() {
        assert_eq!(parse_line("export A_KEY=x"), Some(("A_KEY", "x")));
        assert_eq!(parse_line("  A_KEY = \"x\""), Some(("A_KEY", "\"x\"")));
        assert_eq!(parse_line("use vega x"), None);
        assert_eq!(parse_line("1A=b"), None);
        assert_eq!(classify("\"abc\""), Class::Literal);
        assert_eq!(classify("abc # note"), Class::Literal);
        assert_eq!(classify("$(op read op://Private/x/y)"), Class::Op);
        assert_eq!(classify("$OTHER"), Class::Ref);
        assert_eq!(classify("\"\""), Class::Empty);
        assert_eq!(unquote("'a b' # c"), "a b");
        assert_eq!(unquote("abc # note"), "abc");
        assert_eq!(
            op_ref("$(op read \"op://Private/cli-api-keys/JIRA_API_TOKEN\")").as_deref(),
            Some("op://Private/cli-api-keys/JIRA_API_TOKEN")
        );
    }

    #[test]
    fn rewrite_removes_and_adds_use_once() {
        let text = "#!/bin/sh\nexport A_KEY=x\n# B_KEY=old\nexport PATH=$PATH:bin\nB_KEY=y\n";
        let names = vec!["A_KEY".to_string(), "B_KEY".to_string()];
        let out = rewrite(text, &names, Some("proj"), &[]);
        assert_eq!(
            out,
            "#!/bin/sh\nuse vega proj\n# B_KEY=old\nexport PATH=$PATH:bin\n"
        );
        assert_eq!(rewrite(&out, &names, Some("proj"), &[]), out);
        assert_eq!(rewrite("A_KEY=x\n", &names, None, &[]), "");

        // Aliases go right after the use line, once, also into an existing one.
        let alias = vec!["export B_KEY=\"$C_KEY\"".to_string()];
        let out = rewrite(&out, &names, Some("proj"), &alias);
        assert_eq!(
            out,
            "#!/bin/sh\nuse vega proj\nexport B_KEY=\"$C_KEY\"\n# B_KEY=old\nexport PATH=$PATH:bin\n"
        );
        assert_eq!(rewrite(&out, &[], Some("proj"), &alias), out);
        assert_eq!(classify("\"$C_KEY\""), Class::Ref);
    }

    #[test]
    fn names_from_env() {
        assert_eq!(derive_name("OPENAI_API_KEY"), "openai");
        assert_eq!(derive_name("JIRA_PERSONAL_TOKEN"), "jira-personal");
        assert_eq!(derive_name("PMV_AUTH__API_KEY"), "pmv-auth");
        assert_eq!(derive_name("FAL_KEY"), "fal");
        assert_eq!(derive_name("KEY"), "key");
    }

    #[test]
    fn json_escapes() {
        assert_eq!(json_str("a\"b\\c\n"), r#""a\"b\\c\n""#);
    }

    #[test]
    fn scan_finds_files_and_skips_dirs() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join(".envrc"),
            "export OPENAI_API_KEY=sk\nuse vega p\n",
        )
        .unwrap();
        fs::write(d.path().join(".env.example"), "OPENAI_API_KEY=\n").unwrap();
        fs::create_dir_all(d.path().join("node_modules")).unwrap();
        fs::write(d.path().join("node_modules/.env"), "X_KEY=1\n").unwrap();
        fs::create_dir_all(d.path().join("app")).unwrap();
        fs::write(d.path().join("app/.env"), "DB_PASSWORD=\nDB_URL=x\n").unwrap();
        let files = env_files(d.path(), 3);
        assert_eq!(files, [d.path().join(".envrc"), d.path().join("app/.env")]);
        let text = fs::read_to_string(&files[1]).unwrap();
        let v = scan_text(&files[1], &text);
        assert_eq!(v.len(), 1);
        assert_eq!(
            (v[0].name.as_str(), v[0].class),
            ("DB_PASSWORD", Class::Empty)
        );
    }

    /// An in-memory vault: (title, value).
    #[derive(Default)]
    struct Fake(RefCell<Vec<(String, String)>>);

    impl Vault for Fake {
        fn reference(&self, title: &str) -> String {
            format!("op://T/{title}/credential")
        }
        fn exists(&self, title: &str) -> Result<bool, Error> {
            Ok(self.0.borrow().iter().any(|(t, _)| t == title))
        }
        fn read(&self, r: &str) -> Result<Secret, Error> {
            let b = self.0.borrow();
            let (_, v) = b
                .iter()
                .find(|(t, _)| self.reference(t) == r)
                .expect("known ref");
            Ok(Secret::new(v.clone()))
        }
        fn create(&self, title: &str, _: &str, value: &Secret) -> Result<(), Error> {
            self.0
                .borrow_mut()
                .push((title.into(), value.expose().into()));
            Ok(())
        }
    }

    fn admin_store(dir: &Path) -> (Store, Manifest) {
        let store = Store {
            config: dir.join("c"),
            state: dir.join("s"),
            host: "proteus".into(),
        };
        let (pk, _) = store.host_init().unwrap();
        let mut m = Manifest::default();
        m.set_admin(&["proteus".into()]);
        m.set_host("proteus", &pk);
        (store, m)
    }

    #[test]
    fn adopt_reuses_same_value_and_splits_different_ones() {
        let d = tempfile::tempdir().unwrap();
        let (store, mut m) = admin_store(d.path());
        let vault = Fake::default();
        vault
            .0
            .borrow_mut()
            .push(("openai-api".into(), "sk-1".into()));
        let s = |v: &str| Secret::new(v.into());
        let adopt = |m: &mut Manifest, tool: &str, v: &str| {
            adopt_value(
                &store,
                m,
                &vault,
                "OPENAI_API_KEY",
                tool,
                &s(v),
                None,
                None,
                "f",
            )
        };

        // Same value as the existing 1Password item: reuse it, no new item.
        let a = adopt(&mut m, "psy", "sk-1").unwrap();
        assert_eq!((a.secret.as_str(), a.state), ("openai", "op"));
        assert_eq!(vault.0.borrow().len(), 1);
        assert_eq!(
            m.entry("openai").unwrap().op.as_deref(),
            Some("op://T/openai-api/credential")
        );

        // Same value in another project: one secret, two tools.
        let a = adopt(&mut m, "spellkave", "sk-1").unwrap();
        assert_eq!(
            (a.state, a.tools.join("+")),
            ("existing", "psy+spellkave".into())
        );

        // A different value: its own secret and 1Password item.
        let a = adopt(&mut m, "kmcli", "sk-2").unwrap();
        assert_eq!((a.secret.as_str(), a.state), ("openai-kmcli", "new"));
        assert!(vault.exists("openai-kmcli-api").unwrap());
        assert_eq!(store.decrypt("openai-kmcli").unwrap().expose(), "sk-2");

        // An op ref in the file is kept as the secret's source.
        let a = adopt_value(
            &store,
            &mut m,
            &vault,
            "JIRA_API_TOKEN",
            "rhdh",
            &s("j"),
            None,
            Some("op://Private/cli-api-keys/JIRA_API_TOKEN".into()),
            "f",
        )
        .unwrap();
        assert_eq!((a.secret.as_str(), a.state), ("jira", "op"));
        assert_eq!(
            m.entry("jira").unwrap().op.as_deref(),
            Some("op://Private/cli-api-keys/JIRA_API_TOKEN")
        );

        // The same value under another env name: an alias, no second copy.
        let items = vault.0.borrow().len();
        let a = adopt_value(
            &store,
            &mut m,
            &vault,
            "JIRA_PERSONAL_TOKEN",
            "pensieve",
            &s("j"),
            None,
            None,
            "f",
        )
        .unwrap();
        assert_eq!((a.secret.as_str(), a.state), ("jira", "alias"));
        assert_eq!(
            a.alias().as_deref(),
            Some("export JIRA_PERSONAL_TOKEN=\"$JIRA_API_TOKEN\"")
        );
        assert_eq!(vault.0.borrow().len(), items);
        assert!(m.entry("jira-personal").is_none());
    }

    #[test]
    fn lock_is_exclusive_and_released() {
        let d = tempfile::tempdir().unwrap();
        let (store, _) = admin_store(d.path());
        let l = Lock::acquire(&store).unwrap();
        assert!(store.config.join("secrets.lock").exists());
        drop(l);
        assert!(!store.config.join("secrets.lock").exists());
    }
}
