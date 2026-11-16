//! The process table through `ps -ww -eo pid=,etime=,command=`: which
//! processes are browsers that speak CDP, their profile, port and age.

use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    /// Seconds since the process started, when ps reported it.
    pub age: Option<i64>,
    /// The full command line (argv joined with single spaces).
    pub cmd: String,
}

/// The process table; empty when ps fails (nothing can be attributed then).
pub fn scan() -> Vec<Proc> {
    let Ok(out) = Command::new("ps")
        .args(["-ww", "-eo", "pid=,etime=,command="])
        .output()
    else {
        return Vec::new();
    };
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// `pid`, `etime`, `command` rows, as `ps -o pid=,etime=,command=` prints
/// them: whitespace-split, the command keeps its spaces.
pub fn parse(stdout: &str) -> Vec<Proc> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let age = etime(parts.next()?);
            let cmd = parts.collect::<Vec<_>>().join(" ");
            (!cmd.is_empty()).then_some(Proc { pid, age, cmd })
        })
        .collect()
}

/// ps `etime` (`[[dd-]hh:]mm:ss`) as seconds; None for anything else.
fn etime(s: &str) -> Option<i64> {
    let numeric = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) if numeric(d) => (d.parse::<i64>().ok()?, r),
        Some(_) => return None,
        None => (0, s),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.is_empty() || parts.len() > 3 || !parts.iter().all(|p| numeric(p)) {
        return None;
    }
    let mut total = days * 86400;
    let mut mult = 1;
    for part in parts.iter().rev() {
        total += part.parse::<i64>().ok()? * mult;
        mult *= 60;
    }
    Some(total)
}

/// A browser process that speaks CDP (agent-browser's own launches, the
/// fallback launch, Playwright's bundled Chromium: all carry a
/// remote-debugging flag).
#[derive(Debug, Clone, PartialEq)]
pub struct Browser {
    pub pid: u32,
    pub age: Option<i64>,
    pub headed: bool,
    /// The `--remote-debugging-port=<n>` value (0: assigned by the OS).
    pub port: Option<u16>,
    pub user_data_dir: Option<String>,
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    ChromeForTesting,
    Chromium,
    Chrome,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChromeForTesting => "chrome-for-testing",
            Self::Chromium => "chromium",
            Self::Chrome => "chrome",
        }
    }
}

/// The browser processes in the table.
pub fn browsers(procs: &[Proc]) -> Vec<Browser> {
    procs
        .iter()
        .filter_map(|p| {
            let argv: Vec<&str> = p.cmd.split_whitespace().collect();
            let exe = argv.first()?.to_lowercase();
            if !exe.contains("chrome") && !exe.contains("chromium") {
                return None;
            }
            let debugging = |a: &str| {
                a.starts_with("--remote-debugging-port") || a.starts_with("--remote-debugging-pipe")
            };
            if !argv.iter().any(|a| debugging(a)) {
                return None;
            }
            let port = argv
                .iter()
                .find_map(|a| a.strip_prefix("--remote-debugging-port="))
                .and_then(|v| v.parse().ok());
            let user_data_dir = argv
                .iter()
                .find_map(|a| a.strip_prefix("--user-data-dir=").map(str::to_owned));
            let headed = !argv.iter().any(|a| a.starts_with("--headless"));
            let kind = if p.cmd.to_lowercase().contains("for testing") {
                Kind::ChromeForTesting
            } else if exe.contains("chromium") {
                Kind::Chromium
            } else {
                Kind::Chrome
            };
            Some(Browser {
                pid: p.pid,
                age: p.age,
                headed,
                port,
                user_data_dir,
                kind,
            })
        })
        .collect()
}

/// Whether a live process is our Chromium: started with exactly this CDP
/// port and this profile (the old `chromium.sh`'s `ours`).
pub fn ours(procs: &[Proc], port: u16, profile: &str) -> bool {
    let port_arg = format!("--remote-debugging-port={port}");
    let profile_arg = format!("--user-data-dir={profile}");
    procs.iter().any(|p| {
        let argv: Vec<&str> = p.cmd.split_whitespace().collect();
        argv.contains(&port_arg.as_str()) && argv.contains(&profile_arg.as_str())
    })
}

/// The pids started with exactly this profile dir (the old `cleanup.sh`'s
/// pkill list).
pub fn with_profile(procs: &[Proc], profile: &str) -> Vec<u32> {
    let profile_arg = format!("--user-data-dir={profile}");
    procs
        .iter()
        .filter(|p| {
            let argv: Vec<&str> = p.cmd.split_whitespace().collect();
            argv.contains(&profile_arg.as_str())
        })
        .map(|p| p.pid)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: &str, etime: &str, cmd: &str) -> String {
        format!(" {pid} {etime} {cmd}")
    }

    #[test]
    fn parses_pid_etime_and_full_command() {
        let out = [
            row(
                "4242",
                "05:10",
                "/Applications/Chromium.app/Contents/MacOS/Chromium --flag",
            ),
            row("1", "1-02:03:04", "launchd"),
            " 99 no-time ps".to_owned(),
        ]
        .join("\n");
        let procs = parse(&out);
        assert_eq!(procs.len(), 3);
        assert_eq!(procs[0].pid, 4242);
        assert_eq!(procs[0].age, Some(310));
        assert_eq!(
            procs[0].cmd,
            "/Applications/Chromium.app/Contents/MacOS/Chromium --flag"
        );
        assert_eq!(procs[1].age, Some(93784));
        // An unparseable etime keeps the row (matching by argv needs no age).
        assert_eq!(procs[2].age, None);
        assert_eq!(procs[2].cmd, "ps");
    }

    #[test]
    fn classifies_cdp_browsers() {
        let procs = parse(&[
            row(
                "11",
                "1:00:00",
                "/tmp/cft/chrome-mac/Chrome for Testing.app/Google Chrome for Testing --headless --remote-debugging-port=0 --user-data-dir=/tmp/ab",
            ),
            row(
                "12",
                "2:00",
                "/Applications/Chromium.app/Contents/MacOS/Chromium --remote-debugging-port=9223 --user-data-dir=/data/vega-chromium",
            ),
            row("13", "2:00", "/Applications/Safari.app/Contents/MacOS/Safari --remote-debugging-port=1"),
            row("14", "2:00", "/Applications/Chromium.app/Contents/MacOS/Chromium --no-startup-window"),
        ]
        .join("\n"));
        let browsers = browsers(&procs);
        assert_eq!(browsers.len(), 2);
        assert_eq!(browsers[0].kind, Kind::ChromeForTesting);
        assert!(!browsers[0].headed);
        assert_eq!(browsers[0].port, Some(0));
        assert_eq!(browsers[0].user_data_dir.as_deref(), Some("/tmp/ab"));
        assert_eq!(browsers[1].kind, Kind::Chromium);
        assert!(browsers[1].headed);
        assert_eq!(browsers[1].port, Some(9223));
    }

    #[test]
    fn ours_and_with_profile_match_exact_arguments() {
        let procs = parse(
            &[
                row(
                    "21",
                    "1:00",
                    "Chromium --remote-debugging-port=9223 --user-data-dir=/g",
                ),
                row(
                    "22",
                    "1:00",
                    "Chromium --remote-debugging-port=9224 --user-data-dir=/g2",
                ),
            ]
            .join("\n"),
        );
        assert!(ours(&procs, 9223, "/g"));
        assert!(!ours(&procs, 9224, "/g"));
        assert!(!ours(&procs, 9223, "/g2"));
        assert_eq!(with_profile(&procs, "/g2"), [22]);
    }
}
