Without an ID: list preferences. With an ID alone: read the saved view.
Filter flags replace a view whole. Empty dimensions match everything.
Labels match ANY selected label; machines match ANY selected machine.
--match all (default): a project needs a chosen label AND a chosen machine.
--match any: a chosen label OR a chosen machine is enough (everything on puck plus label vega).
Use hc machine names, or local for terminals on the app host. The built-in all view is reserved.

Examples:
  vega project view Work --label work --machine puck --json
  vega project view Work --label vega --machine puck --match any
  vega project view Work --delete
  vega project view --json
