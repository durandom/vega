#!/bin/sh
# Verify full-history detection without putting a secret in the source tree.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT HUP INT TERM
cp "$root/.gitleaks.toml" "$fixture/"
cd "$fixture"
git init -q
git config user.name ci
git config user.email ci@example.invalid
git config commit.gpgsign false
git config core.hooksPath /dev/null
scan() {
    gitleaks git --config .gitleaks.toml --log-opts=--all --redact --exit-code 1 "$@"
}
git add .gitleaks.toml
git commit -qm 'Clean scanner fixture' --trailer 'Assisted-by: codex'
scan
# Construct a synthetic GitHub token at runtime, so the test itself stays clean.
printf '%s%s\n' 'ghp_' '7Kp2zX9mQ4vR8nT6wY3aB5cD1eF0gHjLsUoI' > planted-secret.txt
git add planted-secret.txt
git commit -qm 'Plant synthetic scanner fixture' --trailer 'Assisted-by: codex'
git rm -q planted-secret.txt
git commit -qm 'Remove synthetic scanner fixture' --trailer 'Assisted-by: codex'
status=0
scan --report-format json --report-path "$fixture/findings.json" > "$fixture/scan.log" 2>&1 || status=$?
if [ "$status" -ne 1 ] || ! grep -q 'github-pat' "$fixture/findings.json"; then
    cat "$fixture/scan.log" >&2
    echo "gitleaks regression: expected github-pat finding and exit 1, got $status" >&2
    exit 1
fi
echo 'gitleaks regression: clean history passes; removed synthetic secret fails (exit 1)'
