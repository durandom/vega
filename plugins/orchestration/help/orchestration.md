# vega orchestration

Project waiting evidence and explicit wait lifecycle; no scheduler or second task database.

    vega orchestration
    vega orchestration --parent <bead>
    vega orchestration --skill
    vega orchestration waiting --on Slayer --for "decision pending" --bead <id>
    vega orchestration clear <wait-id> --evidence "<inspected resolution>"

Flags: --parent <bead> filters waits to that bead and all descendants, including
closed beads; readiness uses bd ready descendants (default: project).
Evidence without a known bead is omitted after successful scope resolution.
Missing parents or failed reads show unknown scope and unfiltered project waits;
readiness remains unknown.
--help/-h prints this help; --version/-v/-V prints the stamped version.

Sources: explicit wait records, registry attention/transcript questions, local and
published post asks (including unanswered expired threads), local UAC parks,
existing local CI receipts and bd ready. Totals describe observed rows, not
proof that unavailable sources are empty. Unpublished remote parks/CI stay unknown.
Unanswered ask metadata expires seven days after its gather deadline; post mail
remains available, and retention expiry is not evidence of an answer.
Question owners come from matching local mode records (headed: Slayer; headless:
parent). Missing/remote mode records leave ownership unknown. Saved machine aliases
are resolved through their SSH targets; unresolved question/park sources stay unknown.
A read acknowledgement never proves an answer, approval, or resolved wait.

Missing/corrupt ask or wait state is unknown; an existing readable empty source
is empty. The home view creates no state, watch or reminder. Explicit waits live
in the coordinator's local SQLite ledger; the registry is a projection. Rows join
by pane@host, bead or thread, without a parallel project-ID protocol.
See waiting --help and clear --help for ownership checks and remote updates.
