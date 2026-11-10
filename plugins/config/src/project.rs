//! Personal project preferences, independent of the terminal manager and registry.
use clap::{error::ErrorKind, CommandFactory, Parser, Subcommand};
use serde_json::{json, Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};
use toml_edit::{value, Array, DocumentMut, Item, Table};
use vega_herdr::{cell, print_failure, Failure};

const GUIDE: &str = include_str!("../PROJECT.md");

#[derive(Parser)]
#[command(
    name = "vega project",
    bin_name = "vega project",
    disable_version_flag = true,
    about = "Personal project display names, labels and sidebar views"
)]
struct Cli {
    /// Print the agent guide for this plugin, SKILL.md form (read by vega skills)
    #[arg(long)]
    skill: bool,
    /// Print the bare version
    #[arg(short = 'V', short_alias = 'v', long)]
    version: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Read all preferences, or read/update one normalized project ID
    #[command(after_help = include_str!("../help/project-config.md"))]
    Config {
        id: Option<String>,
        /// Empty clears the override (use the terminal project label)
        #[arg(long, requires = "id")]
        name: Option<String>,
        /// Add a label; repeatable and idempotent
        #[arg(long, requires = "id")]
        label: Vec<String>,
        /// Remove a label; repeatable and idempotent
        #[arg(long, requires = "id")]
        unlabel: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Read saved views, or replace one view's label and machine filters
    #[command(after_help = include_str!("../help/project-view.md"))]
    View {
        id: Option<String>,
        #[arg(long, requires = "id")]
        label: Vec<String>,
        /// hc machine name; use local for terminals on the app host
        #[arg(long, requires = "id")]
        machine: Vec<String>,
        /// all: a project needs a chosen label and a chosen machine; any: either is enough
        #[arg(long = "match", requires = "id", value_parser = ["all", "any"], conflicts_with = "delete")]
        match_mode: Option<String>,
        #[arg(long, requires = "id", conflicts_with_all = ["label", "machine"])]
        delete: bool,
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::DisplayHelp => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            print_failure(&Failure::new("usage", e.to_string(), "valid usage below"));
            let mut help = Cli::command();
            if let Some(sub) = std::env::args()
                .nth(1)
                .and_then(|s| help.find_subcommand_mut(&s))
            {
                print!("{}", sub.render_long_help());
            } else {
                print!("{}", help.render_long_help());
            }
            return ExitCode::from(2);
        }
    };
    if cli.skill {
        print!("{GUIDE}");
        return ExitCode::SUCCESS;
    }
    if cli.version {
        println!("{}", vega_build::version_line!());
        return ExitCode::SUCCESS;
    }
    let result = config_path().and_then(|path| execute(&path, cli.cmd));
    match result {
        Ok((data, json_output)) => {
            if json_output {
                println!("{data}");
            } else {
                for kind in ["projects", "views"] {
                    if let Some(entries) = data.get(kind).and_then(Value::as_object) {
                        println!("{kind}[{}]{{id,config}}:", entries.len());
                        for (id, config) in entries {
                            println!("  {},{}", cell(id), cell(&config.to_string()));
                        }
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            print_failure(&Failure::new(
                "project_config",
                e,
                "check vega project config --help and projects.toml",
            ));
            ExitCode::from(1)
        }
    }
}

fn config_path() -> Result<PathBuf, String> {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or("HOME or XDG_CONFIG_HOME must be set")?;
    Ok(root.join("vega/projects.toml"))
}

fn strings(item: Option<&Item>) -> Result<Vec<String>, String> {
    let Some(item) = item else {
        return Ok(vec![]);
    };
    item.as_array()
        .ok_or_else(|| "expected an array of strings".to_string())?
        .iter()
        .map(|v| {
            v.as_str()
                .map(String::from)
                .ok_or_else(|| "expected a string".into())
        })
        .collect()
}

fn snapshot(doc: &DocumentMut) -> Result<Value, String> {
    let mut out = json!({"projects": {}, "views": {}});
    for kind in ["projects", "views"] {
        if let Some(entries) = doc.get(kind) {
            let entries = entries
                .as_table_like()
                .ok_or_else(|| format!("{kind}: expected a table"))?;
            let mut rows = Map::new();
            for (id, entry) in entries.iter() {
                let entry = entry
                    .as_table_like()
                    .ok_or_else(|| format!("{kind}.{id}: expected a table"))?;
                let labels = strings(entry.get("labels"))?;
                let row = if kind == "projects" {
                    let name = match entry.get("name") {
                        None => "",
                        Some(v) => v.as_str().ok_or("name: expected a string")?,
                    };
                    json!({"name": name, "labels": labels})
                } else {
                    let mode = match entry.get("match") {
                        None => "all",
                        Some(v) => match v.as_str() {
                            Some(m @ ("all" | "any")) => m,
                            _ => {
                                return Err(format!(
                                    "views.{id}.match: expected \"all\" or \"any\""
                                ))
                            }
                        },
                    };
                    json!({"labels": labels, "machines": strings(entry.get("machines"))?, "match": mode})
                };
                rows.insert(id.to_string(), row);
            }
            out[kind] = Value::Object(rows);
        }
    }
    Ok(out)
}

fn array(values: &[String]) -> Item {
    let mut a = Array::new();
    for s in values {
        a.push(s.as_str());
    }
    value(a)
}

fn execute(path: &Path, cmd: Option<Cmd>) -> Result<(Value, bool), String> {
    let mut match_mode: Option<String> = None;
    let (kind, id, name, labels, unlabels, machines, delete, json_output) = match cmd {
        Some(Cmd::Config {
            id,
            name,
            label,
            unlabel,
            json,
        }) => ("projects", id, name, label, unlabel, vec![], false, json),
        Some(Cmd::View {
            id,
            label,
            machine,
            match_mode: mode,
            delete,
            json,
        }) => {
            match_mode = mode;
            ("views", id, None, label, vec![], machine, delete, json)
        }
        None => ("projects", None, None, vec![], vec![], vec![], false, false),
    };
    if id
        .as_ref()
        .is_some_and(|s| s.trim().is_empty() || (kind == "views" && s == "all"))
    {
        return Err("ID must be nonempty; view 'all' is reserved".into());
    }
    if labels
        .iter()
        .chain(&unlabels)
        .chain(&machines)
        .any(|s| s.trim().is_empty())
    {
        return Err("labels and machines must be nonempty".into());
    }
    let mutate = name.is_some()
        || !labels.is_empty()
        || !unlabels.is_empty()
        || !machines.is_empty()
        || match_mode.is_some()
        || delete;
    // Readers need no writable directory. Writers serialize the read-modify-write,
    // including distinct project and view edits, and atomically replace the file.
    let _lock = if mutate {
        let dir = path.parent().ok_or("missing config directory")?;
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let f = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("projects.lock"))
            .map_err(|e| e.to_string())?;
        f.lock().map_err(|e| e.to_string())?;
        Some(f)
    } else {
        None
    };
    let text = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let before = snapshot(&doc)?;
    if mutate {
        let id = id.as_deref().ok_or("mutation requires an ID")?;
        if doc.get(kind).is_none() {
            doc[kind] = Item::Table(Table::new());
        }
        if delete {
            doc[kind]
                .as_table_like_mut()
                .ok_or("expected table")?
                .remove(id);
        } else {
            if doc[kind].get(id).is_none() {
                doc[kind][id] = Item::Table(Table::new());
            }
            if let Some(name) = name {
                doc[kind][id]["name"] = value(name.trim());
            }
            let mut next = if kind == "projects" {
                strings(doc[kind][id].get("labels"))?
            } else {
                vec![]
            };
            for l in labels {
                if !next.contains(&l) {
                    next.push(l);
                }
            }
            next.retain(|l| !unlabels.contains(l));
            doc[kind][id]["labels"] = array(&next);
            if kind == "views" {
                let mut ms = machines;
                ms.sort();
                ms.dedup();
                doc[kind][id]["machines"] = array(&ms);
                // A view is replaced whole: no --match means the default, all.
                doc[kind][id]["match"] = value(match_mode.as_deref().unwrap_or("all"));
            }
        }
        let after = snapshot(&doc)?;
        if before != after {
            vega_theme_sync::files::write_atomic(path, &doc.to_string())?;
        }
    }
    let mut data = snapshot(&doc)?;
    if let Some(id) = id {
        let row = data[kind].get(&id).cloned().unwrap_or_else(|| {
            if kind == "projects" {
                json!({"name":"","labels":[]})
            } else {
                json!({"labels":[],"machines":[],"match":"all"})
            }
        });
        data = if delete {
            json!({"views": {}})
        } else {
            let mut rows = Map::new();
            rows.insert(id, row);
            let mut out = Map::new();
            out.insert(kind.into(), Value::Object(rows));
            Value::Object(out)
        };
    }
    Ok((data, json_output))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guide() {
        vega_plugin::assert_guide(GUIDE, "project");
    }
    fn config(id: &str, name: Option<&str>, labels: &[&str], unlabels: &[&str]) -> Option<Cmd> {
        Some(Cmd::Config {
            id: Some(id.into()),
            name: name.map(String::from),
            label: labels.iter().map(|s| s.to_string()).collect(),
            unlabel: unlabels.iter().map(|s| s.to_string()).collect(),
            json: true,
        })
    }
    #[test]
    fn temp_config_round_trip_preserves_other_projects_views_and_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/vega/projects.toml");
        assert_eq!(
            execute(&path, None).unwrap().0,
            json!({"projects":{},"views":{}})
        );
        assert!(!path.exists());
        let id = "github.com/team/a";
        execute(&path, config(id, Some("A"), &["work", "work"], &[])).unwrap();
        execute(&path, config("github.com/other/a", Some("Other"), &[], &[])).unwrap();
        let view = Some(Cmd::View {
            id: Some("Work".into()),
            label: vec!["work".into()],
            machine: vec!["puck".into()],
            match_mode: Some("any".into()),
            delete: false,
            json: true,
        });
        execute(&path, view).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("# keep me\nfuture = true\n{text}")).unwrap();
        execute(
            &path,
            config(id, Some(""), &["private"], &["work", "missing"]),
        )
        .unwrap();
        let data = execute(&path, None).unwrap().0;
        assert_eq!(
            data["projects"][id],
            json!({"name":"","labels":["private"]})
        );
        assert_eq!(data["projects"]["github.com/other/a"]["name"], "Other");
        assert_eq!(data["views"]["Work"]["machines"], json!(["puck"]));
        assert_eq!(data["views"]["Work"]["match"], "any");
        assert!(fs::read_to_string(&path)
            .unwrap()
            .starts_with("# keep me\nfuture = true"));
        let saved = fs::read_to_string(&path).unwrap();
        execute(&path, config(id, None, &["private"], &["missing"])).unwrap();
        assert_eq!(saved, fs::read_to_string(&path).unwrap());
    }
    #[test]
    fn malformed_config_is_never_overwritten_and_usage_is_strict() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("projects.toml");
        for text in [
            "not TOML",
            "projects = 2",
            "[projects.a]\nlabels = [3]",
            "[views.a]\nmachines = true",
        ] {
            fs::write(&p, text).unwrap();
            assert!(execute(&p, config("a", Some("x"), &[], &[])).is_err());
            assert_eq!(fs::read_to_string(&p).unwrap(), text);
        }
        assert!(Cli::try_parse_from(["vega-project", "config", "--name", "x"]).is_err());
        assert!(Cli::try_parse_from(["vega-project", "config", "a", "--typo"]).is_err());
    }
}
