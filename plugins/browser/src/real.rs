//! The real agent-browser binary, bypassing vega's shim.

use std::env;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const TOOL: &str = "agent-browser";
/// Tests point this at a fake; it also overrides resolution in production.
pub const ENV: &str = "VEGA_AGENT_BROWSER_BIN";

/// The binary `vega browser` (and nothing else) runs: `ENV` when set, else
/// the first `agent-browser` on PATH outside vega's shim directory. Calling
/// the shim from here would re-record the calling pane as the owner of every
/// session the plugin lists or closes (docs/design/browser.md, Shim).
pub fn real_binary() -> Option<PathBuf> {
    if let Some(p) = env::var_os(ENV).filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.iter().map(|d| d.join(TOOL)).find_map(|candidate| {
        // Run the PATH entry itself, not its link target: a mise shim links
        // to the `mise` binary, which dispatches on argv[0].
        let canonical = candidate.canonicalize().ok()?;
        (is_executable(&canonical) && !in_shim_dir(&canonical)).then_some(candidate)
    })
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Only vega's own shim dir (`~/.local/share/vega/shims/bin`, also reached
/// through links): a path with both a `vega` and a `shims` component.
fn in_shim_dir(path: &Path) -> bool {
    let mut vega = false;
    path.components().any(|c| {
        if c.as_os_str() == "vega" {
            vega = true;
        }
        c.as_os_str() == "shims" && vega
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn skips_the_vega_shims_dir_even_through_links() {
        let tmp = tempfile::tempdir().unwrap();
        let shim_dir = tmp.path().join("vega/shims/bin");
        let real_dir = tmp.path().join("fnm/bin");
        let shim = shim_dir.join(TOOL);
        let real = real_dir.join(TOOL);
        for (dir, file) in [(&shim_dir, &shim), (&real_dir, &real)] {
            fs::create_dir_all(dir).unwrap();
            fs::write(file, "#!/bin/sh\n").unwrap();
            fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        // The PATH entry is a link to the shim dir; canonicalize sees through it.
        let link_dir = tmp.path().join("early");
        std::os::unix::fs::symlink(&shim_dir, &link_dir).unwrap();
        let path = env::join_paths([&link_dir, &real_dir]).unwrap();
        env::set_var("PATH", &path);
        env::remove_var(ENV);
        assert_eq!(real_binary(), Some(real.clone()));
        // A mise-style shim: a link named agent-browser to a multi-call binary.
        let mise_dir = tmp.path().join("mise/shims");
        fs::create_dir_all(&mise_dir).unwrap();
        let mise_shim = mise_dir.join(TOOL);
        std::os::unix::fs::symlink(&real, &mise_shim).unwrap();
        env::set_var("PATH", env::join_paths([&link_dir, &mise_dir]).unwrap());
        assert_eq!(real_binary(), Some(mise_shim));
        env::set_var(ENV, shim.to_string_lossy().as_ref());
        assert_eq!(real_binary(), Some(shim));
        env::remove_var(ENV);
        env::remove_var("PATH");
    }
}
