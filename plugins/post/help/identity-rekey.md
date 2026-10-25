Explicit v2-to-v2 rekey after a project's ID changed (first remote added,
selected remote URL changed). Run it on the affected host with
`vega post relay stop` and the project's writers stopped; it holds the relay
lock and needs this host's completed flag day (complete.json).

Preview resolves the checkout's current ID and plans moving every record of
--from (UAC claim and status, badges, lanes, bd projects/state/git, schedule,
note and PagerDuty receipt projects, hygiene day mark, this host's registry
UAC and pins). It changes nothing; the plan goes outside the state root.
Joins keep all data or fail naming the path: a claim merges only with the
same pane, lanes take the union, caches keep the new ID's record; another
pane, a differing bd record or own pin, or another host's UAC for the new ID
block. A location still resolving to the old ID blocks. An exact inbox-only
registry read or grant of the old ID blocks: that needs orrery and the Slayer.

Apply saves the plan as generation identity-cutover/rekey/<n>.json before any
change, then writes files, replaces the registry keys and verifies them. Run
the same apply again after an interruption. While a generation is incomplete
vegad stays identity quiesced. The flag-day journal and approved.json stay
unchanged. Rollback undoes the newest generation, refusing once its records
changed since; repeat it after an interruption.

Examples:
  vega post identity rekey preview --from local:proteus:/Users/me/src/x.git --output ~/rekey-x.json
  vega post identity rekey apply --plan ~/rekey-x.json
  vega post identity rekey rollback

Details: docs/design/project-identity.md, Rekey after a remote change.
