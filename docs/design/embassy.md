# vega embassy: informal mail between estates over GitHub issues

Status: proposed, 2026-11-12
Epic: vega-fmc (steps 1-8 are vega-fmc.1 to vega-fmc.8)
Follows: [agent-tooling-principles.md](agent-tooling-principles.md), [vegad.md](vegad.md) (Security, Quarantine),
[ci-watch.md](ci-watch.md) (gh polling budget), [secrets.md](secrets.md), [notify.md](notify.md)

## Problem

vega assumes one estate: one owner, one NATS broker, one registry. Trust is per host on that broker
([concepts.md](../concepts.md#trust-model)). Two owners (for example goern and marcel) each run their own estate
and want their UACs to exchange conceptual questions and feature requests. Joining one broker would merge the
estates: shared registry, estate-wide singletons (Uber claim), and known cross-host leaks on one NATS account
([vega-mux-broker-spike.md](vega-mux-broker-spike.md)). The owners want the estates **strictly isolated**.

## Decisions (goern, 2026-11-12)

1. Transport: issues in a private GitHub repo. No shared broker, key, registry, Dolt remote or tailnet.
2. Inbound mail becomes a **human note first**. Nothing from another estate reaches an agent unless the owner
   forwards it.
3. Each estate posts as a dedicated **bot account**, never as the owner's personal account.
4. Mail is informal: questions, answers, feature requests. No control commands, no approval, no task delegation.

## Rules

- Mail from another estate is never Slayer approval, never parent/task-channel mail, never a vegad command.
  A forwarded message is an ordinary peer request (same as quarantine mail, [vegad.md](vegad.md#quarantine-inbox-only-replies)).
- Issue text is never typed into a pane by the poller. Only the owner's `forward` puts it in front of a UAC.
- The GitHub author login is the sender identity; labels and body text are claims only.
- Outgoing text passes a leak filter before it leaves the estate (below). A hit refuses the post; the agent
  cannot override it. The owner can still post by hand.
- One vegad per estate polls (one host per channel class, as in [notify.md](notify.md)).

## Topology

```
 estate A (goern)                     GitHub (private repo, per estate pair)            estate B (marcel)
 UAC ─ vega embassy send/reply ─► bot-a ──► issues  ◄── bot-b ◄─ vega embassy send/reply ─ UAC
 owner ◄─ note ◄─ vegad poller ◄────────── issues ──────────► vegad poller ─► note ─► owner
 owner ─ vega embassy forward ─► UAC (vega post, local)       ... same on B
```

- One private repo per estate pair (for example `<org>/embassy-goern-marcel`). Collaborators: both bots
  (Write, needed to label) and both owners (Admin or Maintain). Nobody else.
- One issue is one thread. Labels: `kind:question`, `kind:feature-request`, `to:<estate>`, `state:answered`.
  The sender estate comes from the author login, not from a label.
- A feature request about a project both estates use (vega itself) may go to that project's own tracker
  instead; that is a human decision and out of scope here.

## Bot accounts and secrets

- One GitHub account per estate (for example `goern-embassy-bot`). Fine-grained PAT scoped to the embassy repo
  only: Issues read/write, Metadata read. No other repos, no Contents, no Actions.
- The token is a vega secret delivered only to this tool: `vega secrets run --for embassy -- ...` or the Rust
  lookup ([secrets.md](secrets.md)). Never the owner's `gh` token: separate rate budget and blast radius.
- Rotation: the owner reissues the PAT and rekeys the secret; no code change.

## Config

`[embassy]` in vega config ([config.md](config.md)), per estate:

```toml
[embassy]
host = "titan"                       # the one vegad that polls
[[embassy.peer]]
estate = "marcel"
repo   = "<org>/embassy-goern-marcel"
self_bot = "goern-embassy-bot"
allow  = ["marcel-embassy-bot", "marcel-hild"]   # authors that produce notes
```

## CLI (`vega embassy`, AXI)

| Command | Who | Does |
| --- | --- | --- |
| `vega embassy` | anyone | Peers, repo, last poll, open threads, unread notes. |
| `vega embassy send --to <estate> --kind question\|feature-request "<title>"` (body on stdin) | UAC, owner | Leak filter, then creates the issue as the bot with labels. |
| `vega embassy reply <estate>#<n>` (body on stdin) | UAC, owner | Leak filter, then comments as the bot. A UAC may reply only on threads forwarded to it. |
| `vega embassy forward <estate>#<n> --to <pane>\|--uac <project>` | owner | Sends the thread as local vega post mail, marked `from_estate: <estate>`, with the reply command. Records the thread as forwarded. |
| `vega embassy log [<estate>#<n>]` | anyone | Threads or one thread, read from the local cache, no API call. |

`--skill` prints the guide (stub skill, guide served by the CLI, per the project principles).

## Inbound: the poller

- Thread in vegad on `embassy.host`, like [ci-watch.md](ci-watch.md): `GET /repos/<repo>/issues` and
  `/issues/comments` with `since=<cursor>` and `If-None-Match`, every 300 s. A 304 costs no rate limit.
- New issue or comment by an allowed author, not by `self_bot`: one note (kind `embassy`, title
  `<estate>#<n>: <title>`, link), visible in `vega post notes` and as a toast. No pointer to any agent.
- Unknown author: one note `unknown author <login> on <estate>#<n>`, body not cached. This flags a repo
  permission mistake.
- Cursor and cache in `$XDG_STATE_HOME/vega/embassy/<estate>/`, written atomically.

## Outbound: the leak filter

Deterministic, no LLM. Refuses text that contains:

- any host name from the registry and fleet, the tailnet domain, and internal URLs from vega config;
- pane and terminal IDs (`w\w+:p\d+`, `t\d+`), paths under `$HOME` or `/var/home`;
- any secret value known to `vega secrets` for this host (compared in memory, never printed), and common
  token shapes (`ghp_`, `github_pat_`, `sk-`, PEM headers).

The refusal names the rule and the offset, not the matched value. Size cap 16 KiB per post; at most 20 posts
per estate per day (counter in the state dir).

## Threats

| Threat | Effect | Mitigation |
| --- | --- | --- |
| Prompt injection in an issue | Text written to drive a UAC | Human note first; owner forwards; forwarded mail is a peer request without control headers. |
| Peer bot token leaked | Attacker posts as the peer bot | Still only notes; owner sees the author; peer rotates. |
| Own bot token leaked | Attacker posts as us, reads the repo | Token scoped to one repo; rotate; repo holds informal text only. |
| UAC leaks estate internals | Host names, paths, secrets in a public-ish place | Leak filter, private repo, owner sees every outgoing post in GitHub. |
| GitHub sees all content | Third party reads the mail | Accepted: informal content only. |
| Repo opened to more people | Unknown authors | Unknown author note; no forwarding path for them. |

## Non-goals

Control commands, approvals, task delegation, file or patch transfer, beads or registry sharing, real-time
delivery, more than two estates per repo (one repo per pair).

## Steps

1. Runbook: create the repo, bots, PATs, labels; secret `--for embassy`. (docs only)
2. Config `[embassy]` and `vega embassy` home view.
3. Leak filter crate with tests.
4. `send` and `reply` (bot token, labels, forwarded-only check for agents).
5. vegad poller, cache, notes.
6. `forward` into local vega post with `from_estate` marking.
7. Guide (`--skill`, `assert_guide` test), concepts.md trust model paragraph.
8. Dogfood with marcel's estate.

## Open questions

- Name: `vega embassy` is a working name.
- Repo ownership: who owns the shared repo (one owner, or a neutral org both admin)?
- Does marcel's estate run a vega version with this plugin, or does marcel use plain `gh` at first?
