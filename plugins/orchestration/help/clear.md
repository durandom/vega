# vega orchestration clear

    vega orchestration clear <wait-id> --evidence "inspected the answer"
    vega orchestration clear <wait-id> --evidence "decision bead closed and reviewed"

Required: <wait-id>, --evidence <text>. --help/-h prints this help.
The ID includes the coordinator host; clear routes to that host through post.
Only the recorded waiter session or its recorded coordinator may clear a wait.
A duplicate clear succeeds without changing the first resolution or evidence.
Unknown IDs, stale identity and unverified ownership fail without settling a wait.
A transport timeout means unknown delivery; inspect and retry the same action.
Resolution evidence records inspection; it does not approve any subsequent action.
