//! After a boot: resume (or offer to resume) agents whose kept hc terminal
//! came back as a shell (vega_herdr::resume; docs/design/hc-migration-inventory.md).
use std::time::Duration;

/// Until hc answers once, the boot pass is retried this often.
const RETRY: Duration = Duration::from_secs(30);

pub fn spawn() {
    std::thread::spawn(|| {
        let mut logged = false;
        loop {
            let mode = vega_herdr::resume::mode();
            match vega_herdr::resume::boot_pass(mode) {
                Ok(None) => return,
                Ok(Some(pass)) => {
                    crate::log(&summary(mode, &pass));
                    return;
                }
                Err(f) if f.code == "hc_not_found" => return,
                Err(f) => {
                    if !std::mem::replace(&mut logged, true) {
                        crate::log(&format!("resume: waiting for hc ({})", f.message));
                    }
                }
            }
            std::thread::sleep(crate::pace::of(RETRY));
        }
    });
}

fn summary(mode: vega_herdr::resume::Mode, pass: &vega_herdr::resume::Pass) -> String {
    let list = |v: &[String]| {
        if v.is_empty() {
            "none".to_string()
        } else {
            v.join(" ")
        }
    };
    format!(
        "resume: boot pass ({mode:?}): resumed {}, offered {}, failed {}",
        list(&pass.resumed),
        list(&pass.offered),
        list(&pass.failed)
    )
}
