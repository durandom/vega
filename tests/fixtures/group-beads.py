"""Create a provisioned coordination clone and a cross-org linked worktree offline."""
import json
from pathlib import Path
import subprocess
import sys

root = Path(sys.argv[1]).resolve()
member_id = "github.com/member-org/service"
coord_id = "github.com/coord-org/coord"


def git(repo, *args):
    subprocess.run(["git", "-C", repo, *args], check=True, capture_output=True)


def repo(name, project):
    checkout = root / name
    checkout.mkdir()
    git(checkout, "init", "-q")
    git(checkout, "remote", "add", "origin", "https://" + project + ".git")
    return checkout


if not (root / "service").exists():
    member = repo("service", member_id)
    git(member, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.org",
        "-c", "commit.gpgSign=false", "commit", "--allow-empty", "-m", "fixture")
    git(member, "worktree", "add", "-b", "sentinel", str(root / "sentinel"))

coord = repo(sys.argv[2], coord_id)
(coord / ".vega").mkdir()
(coord / ".vega/group.toml").write_text(f'members = ["{member_id}"]\n')
data = coord / ".beads/dolt"
data.mkdir(parents=True)
common = str(coord / ".git")
project = dict(key=coord_id, common=common, dir=str(coord), data=str(data),
               database="fixture", remote="http://fixture.invalid/fixture", bd="bd", since=1)
directory = root / "state/vega/bd/projects/v2" / coord_id.encode().hex()
directory.mkdir(parents=True, exist_ok=True)
(directory / (common.encode().hex() + ".json")).write_text(json.dumps(
    dict(schema=2, project=coord_id, common_dir=common, value=project)))
