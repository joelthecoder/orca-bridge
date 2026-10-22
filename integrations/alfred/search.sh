#!/bin/sh
# Alfred passes the complete query as one argv value. Never evaluate it as shell code.
exec "${ORCA_BRIDGE_BIN:-$HOME/.local/bin/orca-bridge}" search "${1:-}" --limit 30 --alfred
