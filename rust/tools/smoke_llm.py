#!/usr/bin/env python3
"""One real LLM call, to prove the key, endpoint, model and validator agree.

Builds a tiny anchored-edit task against a synthetic module in a temp sandbox,
sends it through llm_call.py exactly as the sync pipeline does, runs
validate_port.py on the reply, then checks the file actually changed. Costs a
few thousand tokens -- well under a cent on deepseek-v4.1-flash.

Usage:
  LLM_API_KEY=... python3 rust/tools/smoke_llm.py
  python3 rust/tools/smoke_llm.py --dry-run      # print the prompt, make no call

Reads the same LLM_* environment as llm_call.py, so it tests the configuration
the nightly will use. The key is only ever placed in a request header by
llm_call.py; this script never reads, prints or passes it.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))

PROBE_PATH = "rust/server/src/probe.rs"
PROBE_SOURCE = '''// Probe module used by the LLM smoke test.
pub const PROBE_NAME: &str = "alpha";

pub fn probe_id() -> u32 {
    1
}

pub fn probe_label() -> &'static str {
    PROBE_NAME
}
'''

PROMPT = """You are porting a behavioral change into a Rust module.

The file rust/server/src/probe.rs currently contains:

---FILE---
%s---END FILE---

Change PROBE_NAME from "alpha" to "beta". Nothing else.

Output ONLY a JSON array. Each element is ONE file, in this shape:

  {"path": "rust/server/src/probe.rs",
   "edits": [{"old": "<exact existing text>", "new": "<replacement text>"}]}

The "old" text must match the file exactly, including indentation, and must
occur exactly once. Include at least two lines of context. No prose, no
markdown fences, just the array.
"""

# Published OpenRouter pricing for deepseek-v4.1-flash-1017, USD per token.
PRICE_IN = 0.15 / 1_000_000
PRICE_OUT = 0.60 / 1_000_000


def fail(message):
    print("\nSMOKE: FAIL -- " + message)
    return 1


def main():
    dry_run = "--dry-run" in sys.argv[1:]
    prompt = PROMPT % PROBE_SOURCE
    if dry_run:
        print(prompt)
        print("\nSMOKE: dry run, no request made")
        return 0

    sandbox = tempfile.mkdtemp(prefix="rh-smoke-")
    try:
        os.makedirs(os.path.join(sandbox, "rust/server/src"))
        target = os.path.join(sandbox, PROBE_PATH)
        with open(target, "w", encoding="utf-8") as handle:
            handle.write(PROBE_SOURCE)

        prompt_path = os.path.join(sandbox, "prompt.txt")
        response_path = os.path.join(sandbox, "response.json")
        with open(prompt_path, "w", encoding="utf-8") as handle:
            handle.write(prompt)

        print("model:  %s" % os.environ.get("LLM_MODEL", "(provider default)"))
        print("effort: %s" % (os.environ.get("LLM_REASONING_EFFORT") or "(provider default)"))

        call = subprocess.run(
            [sys.executable, os.path.join(HERE, "llm_call.py"), prompt_path, response_path],
            capture_output=True, text=True)
        print("http:   %s (rc %d)" % (call.stdout.strip() or "000", call.returncode))
        if call.stderr.strip():
            print("detail: %s" % call.stderr.strip().splitlines()[-1])
        if call.returncode != 0:
            return fail("transport failed; see detail above")

        # Report what the call cost before judging the content.
        try:
            with open(response_path, encoding="utf-8") as handle:
                payload = json.load(handle)
            usage = payload.get("usage") or {}
            prompt_tokens = usage.get("prompt_tokens") or 0
            completion_tokens = usage.get("completion_tokens") or 0
            cost = prompt_tokens * PRICE_IN + completion_tokens * PRICE_OUT
            print("tokens: %d in / %d out (~$%.5f)" %
                  (prompt_tokens, completion_tokens, cost))
            finish = (payload.get("choices") or [{}])[0].get("finish_reason")
            print("finish: %s" % finish)
        except Exception as exc:  # noqa: BLE001 - diagnostics only
            print("usage: unreadable (%s)" % exc)

        run = subprocess.run(
            [sys.executable, os.path.join(HERE, "validate_port.py"), response_path, "100000"],
            cwd=sandbox, capture_output=True, text=True)
        token = run.stdout.strip()
        print("validator: %s" % token)
        if token != "applied_1":
            return fail("validator rejected the reply: " + token)

        with open(target, encoding="utf-8") as handle:
            after = handle.read()
        if 'PROBE_NAME: &str = "beta"' not in after:
            return fail("file was written but the edit is not present")
        if "alpha" in after:
            return fail("old value still present")

        print("\nSMOKE: PASS -- key, endpoint and model all work; validator applied the edit")
        return 0
    finally:
        shutil.rmtree(sandbox, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
