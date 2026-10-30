//! `.vega/gate.toml`, read from the push target (gate.md section 2).

use toml_edit::{DocumentMut, Item};

pub const PATH: &str = ".vega/gate.toml";

/// Reviewer candidates when the config names none.
const DEFAULT_REVIEWERS: [&str; 3] = ["codex", "pi:zai", "claude"];

#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    pub name: String,
    pub paths: Vec<String>,
    pub exclude: Vec<String>,
    /// Dir with SKILL.md at the target; `None`: vega's default rubric.
    pub rubric: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub timeout_secs: i64,
    pub tier: String,
    pub reviewers: Vec<String>,
    pub reviews: Vec<Review>,
}

impl Review {
    /// git pathspecs: `:(glob)<path>` for each path, `:(glob,exclude)` for each exclude.
    pub fn pathspecs(&self) -> Vec<String> {
        self.paths
            .iter()
            .map(|p| format!(":(glob){p}"))
            .chain(self.exclude.iter().map(|e| format!(":(glob,exclude){e}")))
            .collect()
    }
}

pub fn parse(text: &str) -> Result<Config, String> {
    let doc = text
        .parse::<DocumentMut>()
        .map_err(|e| format!("{PATH}: {}", e.to_string().lines().next().unwrap_or("")))?;
    let timeout_secs = match doc.get("timeout") {
        None => 20 * 60,
        Some(i) => duration(
            i.as_str()
                .ok_or(format!("{PATH}: timeout is not a string"))?,
        )?,
    };
    let tier = match doc.get("tier") {
        None => "budget".to_string(),
        Some(i) => i
            .as_str()
            .ok_or(format!("{PATH}: tier is not a string"))?
            .to_string(),
    };
    let reviewers = match doc.get("reviewers") {
        None => DEFAULT_REVIEWERS.iter().map(|s| s.to_string()).collect(),
        Some(i) => strings(i, "reviewers")?,
    };
    let Some(tables) = doc.get("review").and_then(Item::as_array_of_tables) else {
        return Err(format!("{PATH}: no [[review]] block"));
    };
    let mut reviews = Vec::new();
    for (n, t) in tables.iter().enumerate() {
        let name = t
            .get("name")
            .and_then(Item::as_str)
            .map(String::from)
            .unwrap_or_else(|| format!("review{}", n + 1));
        let paths = t
            .get("paths")
            .map(|i| strings(i, "paths"))
            .transpose()?
            .unwrap_or_default();
        if paths.is_empty() {
            return Err(format!("{PATH}: review {name} has no paths"));
        }
        let exclude = t
            .get("exclude")
            .map(|i| strings(i, "exclude"))
            .transpose()?
            .unwrap_or_default();
        let rubric = match t.get("rubric") {
            None => None,
            Some(i) => Some(
                i.as_str()
                    .ok_or(format!("{PATH}: rubric of {name} is not a string"))?
                    .trim_end_matches('/')
                    .to_string(),
            ),
        };
        reviews.push(Review {
            name,
            paths,
            exclude,
            rubric,
        });
    }
    Ok(Config {
        timeout_secs,
        tier,
        reviewers,
        reviews,
    })
}

fn strings(item: &Item, key: &str) -> Result<Vec<String>, String> {
    let arr = item
        .as_array()
        .ok_or(format!("{PATH}: {key} is not an array"))?;
    arr.iter()
        .map(|v| {
            v.as_str()
                .map(String::from)
                .ok_or(format!("{PATH}: {key} holds a non-string"))
        })
        .collect()
}

/// `20m`, `90s`, `1h` to seconds.
pub fn duration(s: &str) -> Result<i64, String> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: i64 = num
        .parse()
        .map_err(|_| format!("duration `{s}`: want e.g. 20m"))?;
    match unit {
        "s" => Ok(n),
        "m" | "" => Ok(n * 60),
        "h" => Ok(n * 3600),
        "d" => Ok(n * 86400),
        _ => Err(format!("duration `{s}`: unit s, m, h or d")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_dogfood_config() {
        let c = parse(include_str!("../../../.vega/gate.toml")).unwrap();
        assert_eq!(c.timeout_secs, 1200);
        assert_eq!(c.tier, "budget");
        assert_eq!(c.reviewers[1], "pi:zai/glm-5.3");
        assert_eq!(c.reviews.len(), 1);
        assert_eq!(
            c.reviews[0].rubric.as_deref(),
            Some(".agents/skills/vega-gate-rubric")
        );
        assert!(c.reviews[0]
            .pathspecs()
            .contains(&":(glob,exclude)**/fixtures/**".to_string()));
        // The rubric checks that docs follow code, so the reviewer sees them.
        assert!(c.reviews[0].paths.contains(&"docs/**".to_string()));
    }

    #[test]
    fn defaults_and_errors() {
        let c = parse("[[review]]\npaths = [\"src/**\"]\n").unwrap();
        assert_eq!(c.timeout_secs, 1200);
        assert_eq!(c.reviewers.len(), 3);
        assert_eq!(c.reviews[0].name, "review1");
        assert_eq!(c.reviews[0].rubric, None);
        assert!(parse("timeout = \"20m\"\n").is_err());
        assert!(parse("[[review]]\nname = \"x\"\n").is_err());
        assert!(parse("[[review]\n").is_err());
        assert_eq!(duration("90s"), Ok(90));
        assert!(duration("5w").is_err());
    }
}
