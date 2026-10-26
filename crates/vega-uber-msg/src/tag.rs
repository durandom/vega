//! The ask's tag for the phone (docs/design/uber-uac.md §3): `#` plus the
//! shortest prefix of the id's hash that no other open ask of the Uber
//! uses, 1-4 characters. The hash is the id's own, so the UAC host checks a
//! tag without being told it (§4 check 3).

use sha2::{Digest, Sha256};

const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// The 4 characters a tag is a prefix of: the first 4 bytes of
/// sha256(id), each mod 36 as `[0-9a-z]`.
pub fn hash(id: &str) -> String {
    Sha256::digest(id.as_bytes())[..4]
        .iter()
        .map(|b| DIGITS[(*b % 36) as usize] as char)
        .collect()
}

/// The tag for `id`: the shortest that is not in `taken` (the Uber's other
/// open asks); `None` when all 4 lengths are taken.
pub fn of(id: &str, taken: &[&str]) -> Option<String> {
    let h = hash(id);
    (1..=4)
        .map(|n| format!("#{}", &h[..n]))
        .find(|t| !taken.contains(&t.as_str()))
}

/// Whether `tag` can be the tag of `id`.
pub fn matches(id: &str, tag: &str) -> bool {
    tag.strip_prefix('#')
        .is_some_and(|t| (1..=4).contains(&t.len()) && hash(id).starts_with(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_prefixes_of_the_id_hash() {
        let id = "vega-lhc.209~AbCdEfGhIjKlMnOpQrStUv";
        let h = hash(id);
        assert_eq!(h.len(), 4);
        assert!(h.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert_eq!(hash(id), h, "deterministic");
        let first = of(id, &[]).unwrap();
        assert_eq!(first, format!("#{}", &h[..1]));
        assert!(crate::valid_tag(&first));
        let second = of(id, &[first.as_str()]).unwrap();
        assert_eq!(second, format!("#{}", &h[..2]));
        let all: Vec<String> = (1..=4).map(|n| format!("#{}", &h[..n])).collect();
        let taken: Vec<&str> = all.iter().map(String::as_str).collect();
        assert_eq!(of(id, &taken), None);
        for t in &all {
            assert!(matches(id, t), "{t}");
        }
        assert!(!matches(id, "#"));
        assert!(!matches(id, &h));
        assert!(!matches(id, &format!("#{h}0")));
        let other = (b'0'..=b'z')
            .map(|c| format!("#{}", c as char))
            .find(|t| t != &all[0])
            .unwrap();
        assert!(!matches(id, &other));
    }
}
