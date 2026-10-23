//! Jumping to a herdr pane id clicked in the detail view, the way vega open
//! does (`vega_herdr::pane`): this machine first, then the herdr machines.

use std::sync::mpsc::{channel, Receiver};
use vega_herdr::pane::{self, Hit};

/// The status line and whether the pane is on this machine, or why not.
pub type Outcome = Result<(String, bool), String>;

/// Runs `jump` on a thread; the receiver gets one outcome.
pub fn start(text: String) -> Receiver<Outcome> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(jump(&text));
    });
    rx
}

fn jump(text: &str) -> Outcome {
    let r = pane::parse(text).ok_or_else(|| format!("{text} is no pane id"))?;
    let hits = pane::resolve(
        &r,
        &vega_herdr::local_host(),
        &pane::machines(),
        pane::exists,
    )
    .map_err(|(_, msg)| msg)?;
    let hit = only(&hits)?;
    pane::focus(hit).map_err(|e| format!("{}: {e}", hit.display()))?;
    Ok(match hit {
        Hit::Local(p) => (format!("focused {p}"), true),
        Hit::Remote { machine, .. } => (
            format!(
                "focused {} on {machine}: switch to it in herdr",
                hit.display()
            ),
            false,
        ),
    })
}

/// The board has no machine picker: an id on several machines must be
/// written machine-qualified.
fn only(hits: &[Hit]) -> Result<&Hit, String> {
    match hits {
        [one] => Ok(one),
        _ => Err(format!(
            "on several machines: {}; write it machine-qualified",
            hits.iter().map(Hit::display).collect::<Vec<_>>().join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_machines_ask_for_a_qualified_id() {
        let one = [Hit::Local("w1:p1".into())];
        assert_eq!(only(&one), Ok(&one[0]));
        let remote = |m: &str| Hit::Remote {
            machine: m.into(),
            pane: "w1:p2".into(),
        };
        let e = only(&[remote("io"), remote("deimos")]).unwrap_err();
        assert!(e.contains("io:w1:p2, deimos:w1:p2"), "{e}");
    }
}
