//! Remote URLs in one form, so clones of one repo compare equal.

/// `git@github.com:o/r.git`, `https://github.com/o/r`, `ssh://git@github.com/o/r.git`
/// -> `github.com/o/r`. Local paths stay paths, without a trailing `.git`.
pub fn normalise(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    if let Some((_, rest)) = url.split_once("://") {
        if let Some(rest) = rest.strip_prefix('/') {
            // file:///path
            return format!("/{rest}");
        }
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = host.split(':').next().unwrap_or(host);
        return format!("{}/{}", host.to_lowercase(), path);
    }
    // scp-like: [user@]host:path (not a local path, which has no ':' before '/').
    if let Some((left, path)) = url.split_once(':') {
        if !left.contains('/') {
            let host = left.rsplit_once('@').map_or(left, |(_, h)| h);
            return format!("{}/{}", host.to_lowercase(), path.trim_start_matches('/'));
        }
    }
    url.to_string()
}

/// For humans: GitHub repos as `owner/repo`, others as normalised.
pub fn display(remote: &str) -> &str {
    remote.strip_prefix("github.com/").unwrap_or(remote)
}

/// The repo name: the last path segment.
pub fn repo_name(remote: &str) -> &str {
    remote.rsplit('/').next().unwrap_or(remote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_form_for_every_url_style() {
        for u in [
            "git@github.com:durandom/lanista.git",
            "https://github.com/durandom/lanista",
            "https://github.com/durandom/lanista.git/",
            "ssh://git@github.com/durandom/lanista.git",
            "ssh://git@GitHub.com:22/durandom/lanista",
            "https://user:token@github.com/durandom/lanista.git",
        ] {
            assert_eq!(normalise(u), "github.com/durandom/lanista", "{u}");
        }
        assert_eq!(normalise("/srv/git/x.git"), "/srv/git/x");
        assert_eq!(normalise("file:///srv/git/x.git"), "/srv/git/x");
        assert_eq!(normalise("../x"), "../x");
        assert_eq!(display("github.com/o/r"), "o/r");
        assert_eq!(display("gitlab.com/o/r"), "gitlab.com/o/r");
        assert_eq!(repo_name("github.com/o/r"), "r");
    }
}
