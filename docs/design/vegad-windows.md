# Native vegad on Windows

Extend the Windows worker behind `vega-post.exe relay serve` (vega-nic.28.21).
The user-state lock serializes foreground, scheduled and app-initiated starts.
One worker retains ownership of delivery state and io-win registry keys.

## Worker inventory

| Unix daemon responsibility | Windows disposition |
| --- | --- |
| hc inventory, unread polling, pointer delivery and draft guard | Already native in native_relay.rs. Preserve reconnect and inbox acknowledgement behavior. |
| Registry reconciliation | Native worker publishes the host beat and local agent entries. Defer full reconciliation, identity migration, remote observation and caches. |
| Sidebar refresh | First milestone. Reconcile local hc host, project and role labels independently of NATS. |
| Legacy herdr event subscription, workspace metadata and token renewal | Unix only. hc labels persist until changed or cleared. |
| Broker command dispatch, quarantine, local send listener, gathers, deadlines and child supervision | Later. Keep existing post client routes. |
| Notes, notification delivery, attention, decisions and waits | Later. Require their stores and notification backends. |
| CI watches, beads sync, Git sync, janitor pulls and gate GC | Later. Validate executable discovery, credentials, paths and process cleanup for each. |
| History collection/export, capacity, catalog, upstream watches, PagerDuty and resume | Later. Leave their workers gated individually. |
| Theme following and publication | Later. Keep OS-specific application backends gated. |
| Login keychain probe, Unix service management and self-exec upgrade | Platform-specific. Windows keeps its installer replacement protocol. |

Use configured host glyphs, local checkout identity and UAC/lineage records
with Unix role precedence. Restrict writes to milestone-owned keys. The Unix
sidebar formatter emits empty values for its full key set. Reconcile changed
values, clear a lost role and rewrite desired labels after restart. Failed
inventory retains the last snapshot. Bound hc calls and retry failed writes
without delaying pointer delivery.

## Desktop process and IPC

The herdr-app vega mod starts the relay on connection and checks it every five minutes.
Through `hc exec --on <machine>` the desktop core launches `vega-post.exe relay start`
with its ordinary user token. The starter checks the worker lock. For a cold start it
uses a temporary limited interactive demand task so the worker survives the hc exec job.
Only the desktop core has a logon startup entry (VegaHcDesktopCore, vega-4hyn).
The Windows relay keeps its io-win identity independently of WSL.

Use hc CLI transport (hc owns its named pipe), the native relay's readiness,
lock and token-scoped stop files for lifecycle, and NATS for remote mail.
Milestone 1 needs no listener. For future local socket RPC use a local-only
named pipe scoped to the user SID with a current-user DACL, bounded I/O and
the existing request framing.

## Health, rollout and acceptance

Relay status reports the lock and worker metadata. A fresh io-win registry
beat shows the native polling cycle completed. Verify label writes by reading
the terminal labels. For full acceptance expose last successful inventory and
label reconcile timestamps and the last error locally and in the UAC view.
Age stale health out. Report beads sync separately as unsupported until ported.

1. Exercise local and remote panes, role loss, failed inventory and writes,
   restart, broker outage and unchanged snapshots with a fake hc.
2. Run the Unix checks, Windows CI and native tests in the io scratch checkout.
   Verify actual labels on a disposable desktop terminal in an approved live run.
3. On desktop core connection verify one limited-token worker, fresh health and labels.
   Repeat the starter and verify the same worker retains its lock.
