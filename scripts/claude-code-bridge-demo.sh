#!/usr/bin/env bash
set -euo pipefail

# Demo "claude-code" bridge for codex-web.
#
# This script implements the minimal contract that codex-web expects:
#   claude-code exec [resume <SESSION_ID>] --json <PROMPT>
#
# It does NOT call any real model; it just echoes back the prompt as a JSONL stream.

if [[ "${1:-}" != "exec" ]]; then
  echo "usage: $0 exec [resume <SESSION_ID>] --json <PROMPT>" >&2
  exit 2
fi
shift

session_id="demo-session"
if [[ "${1:-}" == "resume" ]]; then
  shift
  session_id="${1:-demo-session}"
  shift
fi

if [[ "${1:-}" != "--json" ]]; then
  echo "expected --json" >&2
  exit 2
fi
shift

prompt="${1:-}"

json_str() {
  python3 - <<'PY' "$1"
import json, sys
print(json.dumps(sys.argv[1]))
PY
}

printf '{"type":"session_configured","session_id":%s}\n' "$(json_str "$session_id")"
printf '{"type":"assistant_message_delta","delta":"You said: "}\n'
printf '{"type":"assistant_message_delta","delta":%s}\n' "$(json_str "$prompt")"
