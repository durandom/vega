//! Mail metadata is a relay observation, never proof that an inbox is empty.
use serde_json::Value;

pub fn unread(agent: &Value, now: i64) -> String {
    let token = &agent["tokens"]["mail_since"];
    if let Some(ms) = token
        .as_i64()
        .or_else(|| token.as_str()?.parse().ok())
        .filter(|ms| *ms > 0)
    {
        return format!("{} old", age(now.saturating_sub(ms / 1000)));
    }
    if agent["tokens"]["mail"]
        .as_str()
        .is_some_and(|s| !s.is_empty())
    {
        "pending".into()
    } else {
        "unknown".into()
    }
}

fn age(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..172800 => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unread_mail_never_disappears_for_missing_or_malformed_age() {
        for since in [json!(1000000), json!("1000000")] {
            assert_eq!(
                unread(&json!({"tokens":{"mail_since":since}}), 2500),
                "25m old"
            );
        }
        for since in [json!(null), json!("broken"), json!(-1)] {
            assert_eq!(
                unread(&json!({"tokens":{"mail":"✉", "mail_since":since}}), 2500),
                "pending"
            );
        }
        assert_eq!(unread(&json!({}), 2500), "unknown");
        assert_eq!(
            unread(&json!({"tokens":{"mail_since":"3000000"}}), 2500),
            "0s old"
        );
    }
}
