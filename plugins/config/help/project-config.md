Without an ID: all project entries and saved views. With an ID but no mutation flags: read one entry.
Use the full normalized git remote from the vega.project-id terminal label, not a terminal ID.
Missing entries return an empty name and labels, and pinned: false. Empty --name clears the display override.
--pin keeps the project UAC visible in the sidebar Active filter; --unpin clears it.
Both are idempotent, require an ID and are mutually exclusive. Pins respect label/machine views.
Repeated --label adds to the set; --unlabel removes (removal wins). Other entries are preserved.

Examples:
  vega project config --json
  vega project config github.com/team/repo --name 'My project' --label work --pin --json
  vega project config github.com/team/repo --name '' --unlabel work
