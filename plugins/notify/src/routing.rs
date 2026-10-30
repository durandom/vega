use std::{collections::BTreeMap, path::PathBuf};
use toml_edit::{DocumentMut, Item};

#[derive(Clone, Debug)]
pub struct Level {
    pub channels: Vec<String>,
    pub escalate: Vec<String>,
    pub after: i64,
    pub remind: Option<i64>,
}
/// Adapter-specific settings are retained without interpreting credentials here.
#[derive(Clone, Debug, Default)]
pub struct ChannelConfig {
    pub settings: BTreeMap<String, toml_edit::Value>,
    pub projects: Option<Vec<String>>,
}
#[derive(Clone, Debug)]
pub struct Config {
    pub notifier: String,
    pub seat_idle: i64,
    pub desktop: bool,
    pub quiet: Option<(u16, u16)>,
    pub levels: BTreeMap<String, Level>,
    pub channels: BTreeMap<String, ChannelConfig>,
    pub projects: BTreeMap<String, i64>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            notifier: "ganymede".into(),
            seat_idle: 300,
            desktop: false,
            quiet: None,
            levels: BTreeMap::new(),
            channels: BTreeMap::new(),
            projects: BTreeMap::new(),
        }
    }
}
fn duration(s: &str) -> Result<i64, String> {
    let i = s
        .char_indices()
        .last()
        .map(|(i, _)| i)
        .ok_or("empty duration")?;
    let (n, unit) = s.split_at(i);
    let factor = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return Err("duration must use s, m or h".into()),
    };
    n.parse::<i64>()
        .ok()
        .filter(|n| *n > 0)
        .and_then(|n| n.checked_mul(factor))
        .ok_or("invalid duration".into())
}
fn strings(item: &Item) -> Result<Vec<String>, String> {
    item.as_array()
        .ok_or("channels/projects must be an array")?
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty())
                .map(String::from)
                .ok_or("channel/project must be a nonempty string".into())
        })
        .collect()
}
fn minutes(s: &str) -> Result<u16, String> {
    let (h, m) = s.split_once(':').ok_or("quiet must be HH:MM-HH:MM")?;
    let h = h.parse::<u16>().map_err(|_| "invalid quiet hour")?;
    let m = m.parse::<u16>().map_err(|_| "invalid quiet minute")?;
    if h > 23 || m > 59 {
        return Err("invalid quiet time".into());
    }
    Ok(h * 60 + m)
}
impl Config {
    pub fn load() -> Result<Self, String> {
        let root = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .ok_or("no config directory")?;
        match std::fs::read_to_string(root.join("vega/notify.toml")) {
            Ok(s) => Self::parse(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn parse(s: &str) -> Result<Self, String> {
        let doc = s.parse::<DocumentMut>().map_err(|e| e.to_string())?;
        let mut config = Self {
            desktop: true,
            quiet: Some((1350, 450)),
            ..Self::default()
        };
        for (name, initial, escalate, remind) in [
            ("info", vec!["toast"], vec![], None),
            ("action", vec!["toast", "macos"], vec!["ntfy"], Some(3600)),
            ("urgent", vec!["toast", "macos", "ntfy"], vec![], Some(900)),
        ] {
            config.levels.insert(
                name.into(),
                Level {
                    channels: initial.into_iter().map(String::from).collect(),
                    escalate: escalate.into_iter().map(String::from).collect(),
                    after: 900,
                    remind,
                },
            );
        }
        if let Some(v) = doc.get("notifier") {
            let h = v
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("notifier must be a host name")?;
            if vega_herdr::state::host_token(h) != h {
                return Err("notifier must be a host token".into());
            }
            config.notifier = h.into();
        }
        if let Some(v) = doc.get("seat_idle") {
            config.seat_idle = duration(v.as_str().ok_or("seat_idle must be a duration")?)?;
        }
        if let Some(v) = doc.get("quiet") {
            let (a, b) = v
                .as_str()
                .ok_or("quiet must be HH:MM-HH:MM")?
                .split_once('-')
                .ok_or("quiet must be HH:MM-HH:MM")?;
            let interval = (minutes(a)?, minutes(b)?);
            if interval.0 == interval.1 {
                return Err("quiet start and end must differ".into());
            }
            config.quiet = Some(interval);
        }
        if let Some(levels) = doc.get("levels") {
            for (name, item) in levels
                .as_table_like()
                .ok_or("levels must be a table")?
                .iter()
            {
                let level = config
                    .levels
                    .get_mut(name)
                    .ok_or_else(|| format!("unknown level {name}"))?;
                let t = item.as_table_like().ok_or("level must be a table")?;
                for (key, v) in t.iter() {
                    match key {
                        "channels" => level.channels = strings(v)?,
                        "remind" => {
                            level.remind =
                                Some(duration(v.as_str().ok_or("remind must be a duration")?)?)
                        }
                        "escalate" => {
                            let e = v.as_table_like().ok_or("escalate must be a table")?;
                            level.after = duration(
                                e.get("after")
                                    .and_then(Item::as_str)
                                    .ok_or("escalate.after must be a duration")?,
                            )?;
                            level.escalate =
                                strings(e.get("channels").ok_or("escalate.channels required")?)?;
                        }
                        _ => return Err(format!("unknown level setting {key}")),
                    }
                }
            }
        }
        if let Some(channels) = doc.get("channels") {
            for (name, item) in channels
                .as_table_like()
                .ok_or("channels must be a table")?
                .iter()
            {
                let t = item.as_table_like().ok_or("channel must be a table")?;
                let projects = t.get("projects").map(strings).transpose()?;
                let settings = t
                    .iter()
                    .map(|(k, v)| {
                        v.as_value()
                            .cloned()
                            .map(|v| (k.into(), v))
                            .ok_or("channel setting must be a value".into())
                    })
                    .collect::<Result<_, String>>()?;
                config
                    .channels
                    .insert(name.into(), ChannelConfig { settings, projects });
            }
        }
        if let Some(projects) = doc.get("projects") {
            for (name, item) in projects
                .as_table_like()
                .ok_or("projects must be a table")?
                .iter()
            {
                let t = item.as_table_like().ok_or("project must be a table")?;
                let d = t
                    .get("remind")
                    .and_then(Item::as_str)
                    .ok_or("project.remind must be a duration")?;
                config.projects.insert(name.into(), duration(d)?);
            }
        }
        Ok(config)
    }
    pub fn warnings(&self) -> Vec<String> {
        let names: std::collections::BTreeSet<_> = self
            .levels
            .values()
            .flat_map(|l| l.channels.iter().chain(l.escalate.iter()))
            .collect();
        names
            .into_iter()
            .filter(|n| !matches!(n.as_str(), "toast" | "macos") && !self.channels.contains_key(*n))
            .map(|n| {
                format!("channel {n} is named in levels but has no [channels.{n}] configuration")
            })
            .collect()
    }
    pub fn level(&self, level: &str) -> Level {
        self.levels.get(level).cloned().unwrap_or(Level {
            channels: vec!["toast".into()],
            escalate: vec![],
            after: 900,
            remind: None,
        })
    }
    pub fn allowed(&self, name: &str, project: Option<&str>) -> bool {
        if matches!(name, "toast" | "macos" | "uber") {
            return true;
        }
        self.channels.get(name).is_some_and(|c| {
            c.projects
                .as_ref()
                .is_none_or(|ps| project.is_some_and(|p| ps.iter().any(|s| s == p)))
        })
    }
    pub fn is_quiet(&self, now: i64) -> bool {
        let Some((start, end)) = self.quiet else {
            return false;
        };
        local_minute(now).is_none_or(|m| {
            if start < end {
                m >= start && m < end
            } else {
                m >= start || m < end
            }
        })
    }
}
fn local_minute(now: i64) -> Option<u16> {
    let at: libc::time_t = now;
    let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
    // localtime_r writes only to this caller-owned tm and handles DST for this timestamp.
    let ptr = unsafe { libc::localtime_r(&at, tm.as_mut_ptr()) };
    if ptr.is_null() {
        return None;
    }
    let tm = unsafe { tm.assume_init() };
    Some((tm.tm_hour * 60 + tm.tm_min) as u16)
}
