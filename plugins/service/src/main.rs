use std::{env, os::unix::process::CommandExt, process::Command, process::ExitCode};

const GUIDE: &str = include_str!("../SKILL.md");
// One self-contained backend travels both with this binary and panel scans over hc.
const BACKEND: &str = include_str!("../../../config/herdr-app/mods/vega-services/helper.py");

fn main() -> ExitCode {
    let args: Vec<_> = env::args().skip(1).collect();
    match args.as_slice() {
        [arg] if ["--version", "-v", "-V"].contains(&arg.as_str()) => {
            println!("{}", vega_build::version_line!());
            return ExitCode::SUCCESS;
        }
        [arg] if arg == "--skill" => {
            print!("{GUIDE}");
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let error = Command::new("python3")
        .args(["-c", BACKEND, "cli"])
        .args(args)
        .env("VEGA_SERVICE_BIN", env::current_exe().unwrap_or_default())
        .exec();
    println!("error: python3 could not start ({error})\nhelp: install Python 3 and retry vega service ls");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    #[test]
    fn guide() {
        vega_plugin::assert_guide(super::GUIDE, "service");
    }
}
