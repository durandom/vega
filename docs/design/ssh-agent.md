# SSH agent across macOS sessions

Status: accepted · Bead: vega-43s0

hc/herdr terminals run in launchd's Background session. An unlocked login
keychain does not imply that Apple SSH `UseKeychain` can retrieve passphrases
there. A git fetch can prompt repeatedly while the app's keychain lock is green.
The user's Aqua login session can retrieve them and load an agent once.

The dotfiles own the setup; vega diagnoses it and never changes SSH config or
loads keys. A per-user Aqua LaunchAgent `local.ssh-agent` runs `ssh-agent` with
its socket at `~/.ssh/agent.sock` and loads saved keys using Apple's
`ssh-add --apple-load-keychain` against that socket. The user's `~/.ssh/config`
selects `IdentityAgent ~/.ssh/agent.sock`. The socket must stay private to this
user; the agent and loader must run in the Aqua session, not an hc terminal.

If keys are absent, use Terminal.app in the logged-in desktop session:

```sh
SSH_AUTH_SOCK=$HOME/.ssh/agent.sock ssh-add --apple-use-keychain <private-key-path>
SSH_AUTH_SOCK=$HOME/.ssh/agent.sock ssh-add --apple-load-keychain
SSH_AUTH_SOCK=$HOME/.ssh/agent.sock ssh-add -l
```

Enter the passphrase only in the human terminal. Check the LaunchAgent if the
socket is missing or refuses connections. Unlocking the login keychain alone
need not fix Background-session SSH. `ssh-add -l` shows fingerprints/comments;
vega retains and publishes only the number of keys, never those lines.

vegad samples this fixed socket in its existing 60-second macOS keychain loop.
A three-second `ssh-add -l` deadline distinguishes ready (with count), empty,
missing socket, unavailable, timeout and unknown. The existing bounded status
relay carries this optional agent object, including from inbox-only hosts.
Older senders remain readable; older receivers need upgrading to accept the new
field. The app reads the existing cache every five seconds; it adds no agent
processes or timer. Its focused-host lock shows the key count and a tooltip with
socket state and repair hints. Green requires both an unlocked keychain and a
ready agent. Samples older than 180 seconds become unknown, including the count.
The existing human keychain unlock popup is unchanged.

`vega config infra` explicitly probes Git SSH authentication over the network.
It discovers explicit `Host` names for `github.com` and
`gitlab.cee.redhat.com`, plus explicit aliases with a supported `HostName`, in
`~/.ssh/config` and its Includes (relative paths are under `~/.ssh`). A wildcard
alone opts in to no host. Match-only configurations and hosts defined only in
system config are outside discovery. Includes are bounded to 64 files and
execution to 16 aliases. Each check uses user `git`, `ssh -T`, BatchMode, strict
host-key checking, no host-key updates, no connection sharing or local/remote
commands, a five-second connect timeout and an eight-second overall deadline.
GitHub's successful-authentication greeting with exit 1 counts as success, as
does GitLab's welcome greeting with exit 0 or 1; exit 255 never does. Unknown
host keys, VPN/DNS failures and rejected keys fail without prompting. Raw SSH
output is discarded. The checks use the user's configured IdentityAgent so a
misconfigured client is visible even when the fixed socket is healthy.

`--fleet` runs the same diagnostic on each saved hc machine using `machine.exec`
(no terminal creation), then retains the existing registry-name diagnostic.
An unreachable host or missing remote Python/SSH is a failure with a hint.
Local or fleet auth failures exit 1; an empty configured-host set is explicit
and succeeds. Component checks used by onboarding remain local. These are
read-only diagnostics: no config apply, deployment, key loading or service
restart occurs. After deployment, try `vega config infra` and
`vega config infra --fleet`, and hover the lock in the focused host's app pane.

Trust: any process running as this user can ask the agent to authenticate with
loaded keys while logged in. A private socket prevents access by other users;
it does not isolate agents or programs sharing this account. Loaded keys are
available until removed, agent exit or logout. Use the desktop login session
and machine/account boundary accordingly.
