#!/bin/sh
exec "${ORCA_BRIDGE_BIN:-$HOME/.local/bin/orca-bridge}" open "$1" --json
