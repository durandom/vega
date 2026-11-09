Without an ID: all project entries and saved views. With an ID but no mutation flags: read one entry.
Use the full normalized git remote from the vega.project-id terminal label, not a terminal ID.
Missing entries return an empty name and labels. Empty --name clears the display override.
Repeated --label adds to the set; --unlabel removes (removal wins). Other entries are preserved.

Examples:
  vega project config --json
  vega project config github.com/team/repo --name 'My project' --label work --json
  vega project config github.com/team/repo --name '' --unlabel work
