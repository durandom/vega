//! `vega` umbrella CLI (git plugin model).
//!
//! `vega <name> args…` replaces itself with `vega-<name> args…` found on PATH.
//! Bare `vega` lists the plugins it can find.

use std::env;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use vega_plugin::{find_on_path, plugins, PREFIX};

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let Some(name) = args.next() else {
        return home();
    };
    let name = name.to_string_lossy().into_owned();

    match name.as_str() {
        "-v" | "-V" | "--version" => {
            println!("{}", vega_build::version_line!());
            return ExitCode::SUCCESS;
        }
        "-h" | "--help" | "help" => return home(),
        "--skill" => {
            // Removed without alias: each plugin has its own guide behind `vega skills`.
            println!("error{{code,message,hint}}:");
            println!("  removed,vega --skill is removed,run `vega skills` for the guide index");
            return ExitCode::from(2);
        }
        _ => {}
    }

    let bin = format!("{PREFIX}{name}");
    let Some(path) = find_on_path(&bin) else {
        println!("error{{code,message,hint}}:");
        println!("  unknown_plugin,no {bin} on PATH,run `vega` to list plugins");
        return ExitCode::from(2);
    };

    let err = Command::new(&path).arg0(&bin).args(args).exec();
    println!("error{{code,message}}:");
    println!("  exec_failed,{}: {err}", path.display());
    ExitCode::from(1)
}

fn home() -> ExitCode {
    let plugins = plugins();
    println!("bin: {}", collapse_home(env::current_exe().ok()));
    println!("description: Vega: CLI for agent tools; `vega <name>` runs `vega-<name>`");
    if plugins.is_empty() {
        println!("plugins: 0 found on PATH");
        return ExitCode::SUCCESS;
    }
    println!("plugins[{}]:", plugins.len());
    for p in &plugins {
        println!("  {p}");
    }
    println!("help: run `vega <plugin>` for its live view, `vega <plugin> --help` for reference, `vega skills` for the agent guides");
    ExitCode::SUCCESS
}

fn collapse_home(path: Option<PathBuf>) -> String {
    let Some(path) = path else {
        return "unknown".into();
    };
    let s = path.display().to_string();
    match env::var("HOME") {
        Ok(home) if s.starts_with(&home) => format!("~{}", &s[home.len()..]),
        _ => s,
    }
}
