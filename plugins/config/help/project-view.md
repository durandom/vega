Without an ID: list preferences. With an ID alone: read the saved view.
Filter flags replace both dimensions of a view. Empty dimensions match everything.
Labels match ANY selected label; machines match ANY selected machine; dimensions combine with AND.
Use hc machine names, or local for terminals on the app host. The built-in all view is reserved.

Examples:
  vega project view Work --label work --machine puck --json
  vega project view Work --delete
  vega project view --json
