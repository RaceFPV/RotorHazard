#!/bin/bash
# Upstream sync pipeline: fetches upstream, merges, ports Python changes
# to Rust via LLM, tests, and auto-merges when everything passes.
#
# Usage: ./rust/tools/upstream-sync.sh [--dry-run]
#
# Environment:
#   ANTHROPIC_API_KEY   — Claude API key for the LLM port step
#   UPSTREAM_REMOTE     — upstream remote name (default: upstream)
#   SKIP_LLM_PORT       — set to 1 to skip the LLM conversion (merge only)
#
# Files:
#   .upstream-sync-sha  — last successfully synced upstream SHA (created on first run)

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

UPSTREAM_REMOTE="${UPSTREAM_REMOTE:-upstream}"
DRY_RUN="${1:-}"
MARKER_FILE=".upstream-sync-sha"
PORTED_DIRS="src/server src/interface"
LOG_PREFIX="[upstream-sync]"

log() { echo "$LOG_PREFIX $*"; }
die() { echo "$LOG_PREFIX ERROR: $*" >&2; exit 1; }

# ---- fetch upstream ----
log "fetching $UPSTREAM_REMOTE..."
git fetch "$UPSTREAM_REMOTE" 2>/dev/null || die "failed to fetch $UPSTREAM_REMOTE"

UPSTREAM_SHA=$(git rev-parse "$UPSTREAM_REMOTE/main")
CURRENT_BRANCH=$(git branch --show-current)

if [ "$CURRENT_BRANCH" != "rust" ]; then
    die "must be on the 'rust' branch (currently on '$CURRENT_BRANCH')"
fi

# ---- determine what's new ----
if [ -f "$MARKER_FILE" ]; then
    LAST_SHA=$(cat "$MARKER_FILE")
else
    # first run: find the merge-base as the starting point
    LAST_SHA=$(git merge-base HEAD "$UPSTREAM_REMOTE/main")
    log "first run: using merge-base $LAST_SHA"
fi

if [ "$UPSTREAM_SHA" = "$LAST_SHA" ]; then
    log "upstream is at $UPSTREAM_SHA — already synced"
    exit 0
fi

# ---- extract the Python diff (the porting signal) ----
PY_DIFF=$(git diff "$LAST_SHA..$UPSTREAM_SHA" -- $PORTED_DIRS 2>/dev/null || true)
PY_DIFF_FILES=$(git diff --name-only "$LAST_SHA..$UPSTREAM_SHA" -- $PORTED_DIRS 2>/dev/null || true)

if [ -z "$PY_DIFF" ]; then
    log "no changes to ported files ($PORTED_DIRS)"
    echo "$UPSTREAM_SHA" > "$MARKER_FILE"
    log "updated marker to $UPSTREAM_SHA"
    exit 0
fi

CHANGED_FILES=$(echo "$PY_DIFF_FILES" | wc -l)
DIFF_LINES=$(echo "$PY_DIFF" | wc -l)
log "upstream changed $CHANGED_FILES ported file(s), $DIFF_LINES diff lines"
log "changed files:"
echo "$PY_DIFF_FILES" | while read -r f; do log "  $f"; done

if [ "$DRY_RUN" = "--dry-run" ]; then
    log "dry run: would merge and port"
    echo "$PY_DIFF" | head -50
    exit 0
fi

# ---- pre-merge state (for rollback) ----
PRE_MERGE_SHA=$(git rev-parse HEAD)

# ---- git merge upstream ----
log "merging $UPSTREAM_REMOTE/main..."
if ! git merge "$UPSTREAM_REMOTE/main" --no-edit --allow-unrelated-histories 2>/dev/null; then
    log "git merge conflict detected — aborting"
    git merge --abort 2>/dev/null || true

    # create an issue-style report
    CONFLICT_FILES=$(git diff --name-only --diff-filter=U 2>/dev/null || echo "(check manually)")
    cat <<EOF
=== UPSTREAM MERGE CONFLICT ===
Upstream $UPSTREAM_SHA could not be merged cleanly.
Conflicted files:
$CONFLICT_FILES
Pre-merge state: $PRE_MERGE_SHA
Resolve manually, then re-run this script.
EOF
    die "merge conflict — manual resolution required"
fi

log "merge clean — Python changes brought in"

# ---- LLM port step ----
if [ "${SKIP_LLM_PORT:-0}" = "1" ]; then
    log "SKIP_LLM_PORT set — skipping LLM conversion"
    log "Python changes are merged; port them manually to rust/"
    echo "$UPSTREAM_SHA" > "$MARKER_FILE"
    git add "$MARKER_FILE"
    git commit -m "Upstream sync: merged $UPSTREAM_REMOTE/main ($UPSTREAM_SHA), LLM port skipped" --allow-empty
    git push origin rust
    exit 0
fi

if [ -z "${ANTHROPIC_API_KEY:-}" ]; then
    log "ANTHROPIC_API_KEY not set — skipping LLM conversion"
    log "Python changes are merged; port them manually to rust/"
    echo "$UPSTREAM_SHA" > "$MARKER_FILE"
    git add "$MARKER_FILE"
    git commit -m "Upstream sync: merged $UPSTREAM_REMOTE/main ($UPSTREAM_SHA), LLM port skipped" --allow-empty
    git push origin rust
    exit 0
fi

log "porting Python changes to Rust via LLM..."

# Build the LLM prompt: the diff + the current Rust file listing
RUST_FILE_LIST=$(find rust/server/src -name '*.rs' -type f | sort)
PROMPT_FILE=$(mktemp)
cat > "$PROMPT_FILE" <<PROMPT_EOF
You are porting an upstream Python change to the Rust implementation of RotorHazard.

The Rust server is a faithful port of the Python server. It lives in rust/server/src/.
Key modules and their Python counterparts:
- main.rs: server startup, routes (server.py)
- socket.rs: Socket.IO handlers (server.py socket handlers)
- race.rs: race lifecycle (RHRace.py)
- results.rs: leaderboard calculation (Results.py)
- payloads.rs: payload builders (various emit functions)
- api.rs: HTTP JSON API (json_endpoints)
- db.rs: database access (RHData.py / Database.py)
- calibration.rs: calibration engine (calibration.py)
- serial.rs: serial node protocol (serial_node.py + RHInterface.py)
- hardware.rs: hardware update loop (RHInterface.py)
- led.rs: LED effects (led_event_manager.py + rh_led_handler_strip)
- class_rank.rs: class ranking (rh_class_rank_* plugins)
- actions.rs: event actions (EventActions.py + rh_actions_builtin)
- heatgen.rs: heat generators (rh_heatgenerator_*)
- mock.rs: mock signal providers (MockSignal.py)
- cluster.rs: cluster management (ClusterNodeSet.py)
- vrx.rs: VRx control framework (VRxControl.py)
- pluginrpc.rs: plugin bridge RPC (RHAPI surface)
- sensor.rs: Linux sensors (linux_sensor.py)
- state.rs: shared state (RHRace state + config)
- phase3.rs: phase 3 features (Language.py etc.)
- pages.rs: page rendering (PageCache.py)
- export_import.rs: data export/import (data_export/import.py)
- util.rs: utilities (RHUtils.py)

Here is the upstream Python diff:
---BEGIN PYTHON DIFF---
$PY_DIFF
---END PYTHON DIFF---

Here are the Rust source files that may need changes:
$(for f in $RUST_FILE_LIST; do echo "=== $f ==="; cat "$f" | head -100; echo "... (truncated)"; done)

Port the Python behavioral changes to the Rust code. Output ONLY a JSON
array of file edits, each with "path" and "new_content" (the full updated
file). If a change doesn't apply to the Rust port (Python-only concern),
skip it. Be conservative: match existing Rust patterns and style.
PROMPT_EOF

# Call the Claude API
LLM_OUTPUT_FILE=$(mktemp)
HTTP_STATUS=$(curl -s -w '%{http_code}' -o "$LLM_OUTPUT_FILE" \
    https://api.anthropic.com/v1/messages \
    -H "Content-Type: application/json" \
    -H "x-api-key: $ANTHROPIC_API_KEY" \
    -H "anthropic-version: 2023-06-01" \
    -d "$(python3 -c "
import json, sys
with open('$PROMPT_FILE') as f:
    prompt = f.read()
print(json.dumps({
    'model': 'claude-sonnet-4-20250514',
    'max_tokens': 16000,
    'messages': [{'role': 'user', 'content': prompt}]
}))
")" 2>/dev/null) || HTTP_STATUS="000"

if [ "$HTTP_STATUS" != "200" ]; then
    log "LLM API call failed (HTTP $HTTP_STATUS)"
    cat "$LLM_OUTPUT_FILE" | head -5
    log "Python changes are merged; port them manually"
    echo "$UPSTREAM_SHA" > "$MARKER_FILE"
    git add "$MARKER_FILE"
    git commit -m "Upstream sync: merged $UPSTREAM_REMOTE/main ($UPSTREAM_SHA), LLM port failed (HTTP $HTTP_STATUS)" --allow-empty
    git push origin rust
    rm -f "$PROMPT_FILE" "$LLM_OUTPUT_FILE"
    exit 0
fi

# Extract the LLM's file edits from the response
log "applying LLM port output..."
PORT_APPLIED=$(python3 - <<'PYEOF'
import json, sys

with open("$LLM_OUTPUT_FILE") as f:
    response = json.load(f)

text = response.get("content", [{}])[0].get("text", "")

# extract JSON array from the LLM output (may be wrapped in markdown)
import re
match = re.search(r'\[.*\]', text, re.DOTALL)
if not match:
    print("no_edits")
    sys.exit(0)

try:
    edits = json.loads(match.group())
except json.JSONDecodeError:
    print("parse_error")
    sys.exit(0)

applied = 0
for edit in edits:
    path = edit.get("path", "")
    content = edit.get("new_content", "")
    if path and content and path.startswith("rust/server/src/"):
        with open(path, "w") as f:
            f.write(content)
        applied += 1
        print(f"applied: {path}")

print(f"applied_{applied}")
PYEOF
) || PORT_APPLIED="script_error"

rm -f "$PROMPT_FILE" "$LLM_OUTPUT_FILE"

if [[ "$PORT_APPLIED" == "no_edits" ]] || [[ "$PORT_APPLIED" == "parse_error" ]] || [[ "$PORT_APPLIED" == "script_error" ]]; then
    log "LLM port: $PORT_APPLIED — no Rust changes applied"
    log "Python changes are merged; port them manually"
    echo "$UPSTREAM_SHA" > "$MARKER_FILE"
    git add -A
    git commit -m "Upstream sync: merged $UPSTREAM_REMOTE/main ($UPSTREAM_SHA), LLM port: $PORT_APPLIED" --allow-empty
    git push origin rust
    exit 0
fi

log "LLM applied $PORT_APPLIED"

# ---- test gate ----
log "running cargo test..."
if ! cargo test --release --manifest-path rust/server/Cargo.toml 2>&1 | tail -5 | grep -q 'test result: ok'; then
    log "TESTS FAILED — reverting to pre-merge state"
    git reset --hard "$PRE_MERGE_SHA"
    log "creating failure report..."

    ISSUE_BODY="Upstream sync from $UPSTREAM_SHA failed tests after LLM port.
Pre-merge SHA: $PRE_MERGE_SHA
Python diff:
\`\`\`
$PY_DIFF
\`\`\`
LLM applied: $PORT_APPLIED
Test output available in the workflow log."
    log "ISSUE: $ISSUE_BODY"
    # if gh CLI is available, create an issue
    if command -v gh &>/dev/null; then
        gh issue create --title "Upstream sync $UPSTREAM_SHA: tests failed" --body "$ISSUE_BODY" 2>/dev/null || true
    fi
    die "tests failed — reverted, issue created"
fi

# ---- success: update marker and push ----
log "tests passed"
echo "$UPSTREAM_SHA" > "$MARKER_FILE"
git add "$MARKER_FILE"
git commit -m "Upstream sync: merged + ported $UPSTREAM_REMOTE/main ($UPSTREAM_SHA) [auto]" \
    --allow-empty 2>/dev/null || true

if git push origin rust 2>/dev/null; then
    log "pushed to origin/rust — sync complete"
else
    log "push failed — changes committed locally"
fi

log "done"
