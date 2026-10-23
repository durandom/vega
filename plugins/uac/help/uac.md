Usage: vega uac

Coordinator mode. Without arguments: every agent working on this session's
project ID (the normalized Git fetch remote), across local clones and fresh
schema-2 registry rows on other hosts, with machine, pane, name, kind,
status, unread_mail, parent, mode and the claimed bead. Agents whose parent
is your pane are your sentinels. Registry-off mode omits remote membership;
remote paths are metadata and never opened locally.

Spawn and adopt default to headed. For unattended sentinels, use
`vega mux spawn --headless` or `vega uac adopt --headless` so questions go to
the parent. Older children without a valid mode record remain headless.

attention (from vegad and `vega post done`): input (blocked on a question or
approval), failure (done --outcome failed), silent (idle after its prompt
without done), quota (its turn ended on a provider abort: overloaded, retry
soon, or quota exhausted, retry after the reset or move the work to another
kind/provider), exited (pane gone), completed (done --outcome succeeded, waits
for you to land or release it), parked (waits on a decision bead: no wake),
decided (its decision bead closed: vegad woke you to resume it). A headed
pane's asking, silent and input are for the user working in it: listed with
next `for the user (headed pane; no wake)`, but vegad does not wake you for them. The
attention table adds the done report (parked: the bead) and the next command. Local input shows the dialog kind and title; all_done when every sentinel is completed.

Commands:
  land BEAD --branch BRANCH [--head SHA] [--target BRANCH] [--keep]
               resumable reviewed landing: merge, check, gate, verified push,
               install, bead close and owned sentinel cleanup; never deploys
               land BEAD --resume | --status; see vega uac land --help
  prune [--dry-run]  safely clean merged branches and clean paneless worktrees;
               see vega uac prune --help
  ask "<q>"    one question to every other agent on the project (all machines);
               the answers come back as one pointer when all answered, or at the
               deadline (--within, default 10m); read them with vega post inbox
  claim        register this session as the project's one UAC (refused while
               another UAC of the project is alive; a gone one is taken over)
  release      hand the project back (the UAC only)
  adopt <pane> [--headless]
               make a session you did not spawn your sentinel: you become its
               parent, vegad supervises it, and it is told to report to you;
               headed (the user works in it, its dialogs stay) unless
               --headless (its questions go to you); flip: vega mux mode
  status       the report for the Slayer: the view above, the GitHub core limit
               (shared by every agent and CI job of the user) and gh calls per
               pane in the last hour (gh shim), the beads sync per project
               (vega bd: ahead, behind, last sync, state), then what landed since
               the last status (closed beads, default-branch commits), the lane
               backlog, then the open decisions (beads labelled slayer-decision),
               answered ones first; records the time. capacity: per provider
               (token-burn's stored sample) the binding window (its account
               window with the least left), percent left,
               reset, when a confident forecast runs it out before the reset, and
               the Slayer's resets owned (vega capacity); a planned reset or an
               override replaces the `(soon)` mark and the spawn warning;
               `capacity: unknown (<why>)` without token-burn. Lane backlog: beads your
               sentinels' worktrees created (bd actor <host>/<worktree dir>, since
               spawn or adopt) get the label uac-lane; the line counts them open,
               created and closed since then, and names the oldest open one
  capacity [...]  alias of `vega capacity [...]`: every provider window, the
               Slayer's resets ledger (resets, plan, used, override) and
               token-burn's health; see `vega capacity --help`
  answer <bead> "<a>"  the Slayer's answer to a decision: a bead comment (author
               Slayer) plus mail to the UAC, which acts and closes the bead
  park <pane> --bead <id> [--machine <m>]
               a sentinel waits on an open decision bead: vegad stops waking
               you for it and wakes you once when the bead closes; new work
               you send it unparks it
  unpark <pane> [--machine <m>]  ends the park without waiting for the bead
  rules        the generic coordinator rules (briefing, branches, landing, cleanup)

Flags:
  --skill      the coordinator role guide (also `vega skills uac`)
  -h, --help   this help
  --version    the version

Examples:
  vega uac              # the facility view for this project
  vega uac status       # the report: running, attention, landed, decisions
  vega uac ask "Your task, branch, bead and blockers? One line each."
  vega uac park w3V:p2 --bead sk-w8zx.7   # waits on that decision
  vega capacity plan claude --this-week   # the Slayer resets claude this week
  vega skills uac        # the role guide
  vega uac rules        # the coordinator rules
