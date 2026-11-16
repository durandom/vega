#!/bin/sh
# Migration (docs/design/browser.md): forwards the old browser skill's
# launcher to the plugin; goes once every hard-coded caller switched.
exec vega browser chromium "$@"
