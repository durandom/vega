//! Attempt-local timing; no history state participates in landing recovery.
use super::*;
use std::time::Instant;

pub(super) struct Trace {
    operation: String,
    attempt: String,
    started: Instant,
    phase_started: Instant,
    phase: &'static str,
    cwd: PathBuf,
    pub events: Vec<(&'static str, Value)>,
}

pub(super) fn operation(dir: &Path, bead: &str) -> String {
    vega_herdr::history::hash(&format!("{}:{bead}", dir.display()))
}

impl Trace {
    pub fn new(dir: &Path, r: &Receipt, started: Instant) -> Self {
        Self {
            operation: operation(dir, &r.bead),
            attempt: vega_herdr::history::event("", "land", Value::Null)["id"]
                .as_str()
                .unwrap()
                .into(),
            started,
            phase_started: started,
            phase: "preflight",
            cwd: dir.into(),
            events: Vec::new(),
        }
    }

    fn data(&self, r: &Receipt, result: &Result<()>, duration: u128) -> Value {
        json!({"operation": self.operation, "attempt": self.attempt,
            "cwd": self.cwd, "bead": r.bead, "branch": r.branch,
            "head": r.head, "commit": r.integrated, "target": r.policy.target,
            "phase": self.phase, "success": result.is_ok(), "duration_ms": duration,
            "check": if result.is_err() && self.phase == "check" { "not_passed" } else if r.checked { "exit 0" } else { "pending" },
            "gate": if result.is_err() && self.phase == "gate" { "not_passed" } else { &r.gate },
            "gate_evidence": r.gate, "install": live::install_result(r),
            "renumbered": r.renumbered,
            "error": result.as_ref().err().map(|e| json!({"message":e.message,"exit":e.status}))})
    }

    pub fn next(&mut self, phase: &'static str, r: &Receipt) {
        self.events.push((
            "land.phase",
            self.data(r, &Ok(()), self.phase_started.elapsed().as_millis()),
        ));
        self.phase = phase;
        self.phase_started = Instant::now();
    }

    pub fn finish(&mut self, dir: &Path, r: &Receipt, result: &Result<()>) {
        self.events.push((
            "land.phase",
            self.data(r, result, self.phase_started.elapsed().as_millis()),
        ));
        let mut data = self.data(r, result, self.started.elapsed().as_millis());
        // Git paths are evidence only; failure to inspect never affects landing.
        data["conflicts"] = git(dir, &["diff", "--name-only", "--diff-filter=U", "-z"])
            .ok()
            .map(|s| json!(s.split('\0').filter(|s| !s.is_empty()).collect::<Vec<_>>()))
            .unwrap_or(Value::Null);
        data["completed"] = json!(result.is_ok() && r.phase == "done");
        self.events.push(("land", data));
    }
}
