#!/bin/sh
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
UI_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
LOCAL_BIN_DIR="$UI_DIR/.tools/bin"

if [ "${NO_COLOR:-}" = "1" ]; then
  export NO_COLOR=true
fi

if [ -d "$LOCAL_BIN_DIR" ]; then
  export PATH="$LOCAL_BIN_DIR:$PATH"
fi

exec trunk build --config Trunk.toml
