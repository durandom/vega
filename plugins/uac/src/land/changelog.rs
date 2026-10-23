//! Resolve only additive parallel blocks, preserving all published bytes.
use super::*;

fn first_block(text: &str) -> Option<usize> {
    text.match_indices("## ")
        .find(|(i, _)| *i == 0 || text.as_bytes()[i - 1] == b'\n')
        .map(|(i, _)| i)
}
fn number(header: &str) -> Option<u64> {
    let rest = header.strip_prefix("## ")?;
    let (n, date) = rest.split_once(" · ")?;
    if date.len() != 10
        || date.bytes().enumerate().any(|(i, c)| {
            if [4, 7].contains(&i) {
                c != b'-'
            } else {
                !c.is_ascii_digit()
            }
        })
    {
        return None;
    }
    n.parse().ok()
}
fn blocks(text: &str) -> Option<Vec<(u64, &str)>> {
    let mut positions: Vec<_> = text
        .match_indices("## ")
        .filter(|(i, _)| *i == 0 || text.as_bytes()[i - 1] == b'\n')
        .map(|(i, _)| i)
        .collect();
    if positions.first() != Some(&0) {
        return None;
    }
    positions.push(text.len());
    let mut out = Vec::new();
    for pair in positions.windows(2) {
        let block = &text[pair[0]..pair[1]];
        out.push((number(block.lines().next()?)?, block));
    }
    if out.windows(2).any(|p| p[0].0 <= p[1].0) {
        return None;
    }
    Some(out)
}
fn combine(base: &str, ours: &str, theirs: &str) -> Option<(String, Vec<(u64, u64)>)> {
    let i = first_block(base)?;
    let prefix = &base[..i];
    let suffix = &base[i..];
    let base_max = blocks(suffix)?.first()?.0;
    let additions = |text: &str| -> Option<String> {
        Some(text.strip_prefix(prefix)?.strip_suffix(suffix)?.to_string())
    };
    let ours_new = additions(ours)?;
    let theirs_new = additions(theirs)?;
    let o = blocks(&ours_new)?;
    let t = blocks(&theirs_new)?;
    if o.last()?.0 <= base_max || t.last()?.0 <= base_max {
        return None;
    }
    let max = o.first()?.0;
    let mut incoming = String::new();
    let mut renumbered = Vec::new();
    for (i, (old, block)) in t.iter().enumerate() {
        let new = max.checked_add((t.len() - i) as u64)?;
        incoming.push_str(&block.replacen(&format!("## {old} ·"), &format!("## {new} ·"), 1));
        renumbered.push((*old, new));
    }
    Some((format!("{prefix}{incoming}{ours_new}{suffix}"), renumbered))
}

pub(super) fn resolve(dir: &Path, path: &Path, r: &mut Receipt) -> Result<()> {
    let paths = git(dir, &["diff", "--name-only", "--diff-filter=U"])?;
    let staged = paths.is_empty() && !r.renumbered.is_empty();
    if paths.is_empty() && !staged {
        return Err(Fault::new(format!("MERGE_HEAD exists but no unmerged paths remain; inspect staged changes, then git commit --no-edit --trailer \"Assisted-by: {}\" and vega uac land {} --resume", r.assisted_by, r.bead)));
    }
    if paths != "CHANGELOG.md" && !staged {
        return Err(Fault::new(format!("merge conflict requires UAC resolution: {paths}; preserve MERGE_HEAD and resume after reviewed resolution")));
    }
    let base_sha = git(dir, &["merge-base", &r.before, &r.head])?;
    let base = command(dir, "git", &["show", &format!("{base_sha}:CHANGELOG.md")])?;
    let ours = command(dir, "git", &["show", &format!("{}:CHANGELOG.md", r.before)])?;
    let theirs = command(dir, "git", &["show", &format!("{}:CHANGELOG.md", r.head)])?;
    let (text, renumbered) = combine(&base, &ours, &theirs).ok_or_else(|| {
        Fault::new("CHANGELOG conflict is not solely parallel additive blocks; resolve manually")
    })?;
    if staged
        && (command(dir, "git", &["show", ":CHANGELOG.md"])? != text
            || fs::read_to_string(dir.join("CHANGELOG.md"))? != text)
    {
        return Err(Fault::new(
            "staged changelog differs from recorded resolution; inspect manually",
        ));
    }
    r.renumbered = renumbered;
    // Persist the disposition before changing the index or creating a commit.
    save(path, r)?;
    fs::write(dir.join("CHANGELOG.md"), text)?;
    git(dir, &["add", "--", "CHANGELOG.md"])?;
    git(
        dir,
        &[
            "commit",
            "--no-edit",
            "--trailer",
            &format!("Assisted-by: {}", r.assisted_by),
        ],
    )?;
    r.integrated = Some(sha(dir, "HEAD")?);
    current(dir, r)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_published_bytes_and_renumbers_multiple_incoming_blocks() {
        let base = "# Changelog\n\n## 1 · 2026-10-20\n\n- old\n";
        let ours = base.replacen("## 1", "## 2 · 2026-10-21\n\n- ours\n\n## 1", 1);
        let theirs = base.replacen(
            "## 1",
            "## 3 · 2026-10-21\n\n- second\n\n## 2 · 2026-10-21\n\n- first\n\n## 1",
            1,
        );
        let (merged, map) = combine(base, &ours, &theirs).unwrap();
        assert_eq!(map, [(3, 4), (2, 3)]);
        assert!(merged.ends_with(ours.strip_prefix("# Changelog\n\n").unwrap()));
        assert!(merged.contains("## 4 · 2026-10-21\n\n- second"));
        assert!(combine(base, &ours.replace("- old", "- edited"), &theirs).is_none());
        assert!(combine(base, &ours, &theirs.replace("# Changelog", "# Changed")).is_none());
    }
}
