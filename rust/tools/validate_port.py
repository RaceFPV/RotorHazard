#!/usr/bin/env python3
"""Validate and apply the Rust edits in an upstream-sync LLM response.

Usage: validate_port.py <response.json> <sanitized_diff_bytes>

Always exits 0 and prints exactly one result token; upstream-sync.sh keys off
the token:

  applied_<n>              edits validated and written
  no_edits                 response held no JSON array
  parse_error              response or JSON array did not parse
  size_exceeded            output far larger than the Python diff
  bad_path:<p>             edit outside the Rust tree
  empty_content:<p>        edit blanked a file
  truncated:<p>            output lost most of an existing file (token cap)
  blocked:<kind>:<rule>:<p> an added line violated a rule

Rules run on ADDED LINES ONLY. Scanning whole file bodies rejects files that
legitimately contain a banned word -- auth modules say `PASSWORD`, asset
loaders say `include_str!`, the Java bridge says `std::process::Command` -- so
a body-wide scan can never pass on real modules. Comparing old and new file
content keeps the same teeth without the false positives.
"""

import difflib
import json
import os
import re
import sys

ALLOWED_PREFIXES = ("rust/server/src/", "rust/server/tests/")

# Structural rules: must never appear in newly added Rust code.
STRUCTURAL = [
    (r"\bunsafe\s*\{", "unsafe_block"),
    (r"std::process::Command", "process_spawn"),
    (r"std::net::(?:Tcp|Udp)", "raw_socket"),
    (r"include_str!|include_bytes!", "include_file"),
    (r"extern\s+crate", "extern_crate"),
    (r"/bin/(?:sh|bash)", "shell_path"),
    (r"https?://[a-z0-9]", "url"),
]

# Credential-shaped additions: a hardcoded secret, not an identifier.
SECRETS = [
    (r"(?:sk-[A-Za-z0-9]{16,}|ghp_[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{12,}|xox[bp]-[A-Za-z0-9-]{10,})",
     "key_prefix"),
    (r"(?i)\b(?:api[_-]?key|secret|password|token)\w*\s*[:=]\s*\"[^\"]{12,}\"",
     "literal_credential"),
]

# Prompt-injection shaped additions.
INJECTION = [
    (r"(?i)ignore\s+(?:all\s+)?(?:the\s+)?(?:previous|above|prior)\s+instructions", "ignore_instructions"),
    (r"(?i)disregard\s+(?:the\s+)?(?:system|previous|above)", "disregard"),
    (r"(?i)you\s+are\s+(?:now\s+)?an?\s+(?:ai|assistant|llm|language model)", "authority_claim"),
    (r"(?i)system\s*prompt", "system_prompt"),
]

RULESETS = (
    ("structural", STRUCTURAL),
    ("secret", SECRETS),
    ("injection", INJECTION),
)

# A re-emit shorter than this fraction of the original almost always means the
# response was cut off by max_tokens, which silently mangles big modules.
TRUNCATION_RATIO = 0.5
TRUNCATION_MIN_BYTES = 400


def added_lines(old_text, new_text):
    """Lines present in new_text that were not in old_text."""
    old = old_text.splitlines()
    new = new_text.splitlines()
    matcher = difflib.SequenceMatcher(None, old, new, autojunk=False)
    added = []
    for tag, _i1, _i2, j1, j2 in matcher.get_opcodes():
        if tag in ("insert", "replace"):
            added.extend(new[j1:j2])
    return added


def response_text(response):
    blocks = response.get("content") or []
    parts = []
    for block in blocks:
        if isinstance(block, str):
            parts.append(block)
        elif isinstance(block, dict) and block.get("type", "text") == "text":
            parts.append(block.get("text") or "")
    return "".join(parts)


def main():
    response_path = sys.argv[1]
    sanitized_size = int(sys.argv[2] or 0)

    try:
        with open(response_path, encoding="utf-8", errors="replace") as handle:
            response = json.load(handle)
    except Exception:
        print("parse_error")
        return

    text = response_text(response)
    match = re.search(r"\[[\s\S]*\]", text)
    if not match:
        print("no_edits")
        return
    try:
        edits = json.loads(match.group())
    except json.JSONDecodeError:
        print("parse_error")
        return
    if not isinstance(edits, list) or not edits:
        print("no_edits")
        return

    total_size = 0
    for edit in edits:
        if isinstance(edit, dict):
            total_size += len(edit.get("new_content") or "")
    if sanitized_size > 0 and total_size > sanitized_size * 20:
        print("size_exceeded")
        return

    staged = []
    for edit in edits:
        if not isinstance(edit, dict):
            print("parse_error")
            return
        path = (edit.get("path") or "").replace("\\", "/")
        content = edit.get("new_content") or ""

        if not path.startswith(ALLOWED_PREFIXES) or ".." in path:
            print("bad_path:" + path)
            return
        if not content.strip():
            print("empty_content:" + path)
            return

        old_text = ""
        if os.path.exists(path):
            with open(path, encoding="utf-8", errors="replace") as handle:
                old_text = handle.read()
            if (len(old_text) > TRUNCATION_MIN_BYTES
                    and len(content) < len(old_text) * TRUNCATION_RATIO):
                print("truncated:" + path)
                return

        lines = added_lines(old_text, content)
        for kind, ruleset in RULESETS:
            for pattern, rule in ruleset:
                for line in lines:
                    if re.search(pattern, line):
                        print("blocked:%s:%s:%s" % (kind, rule, path))
                        return

        staged.append((path, content))

    for path, content in staged:
        parent = os.path.dirname(path)
        if parent:
            os.makedirs(parent, exist_ok=True)
        with open(path, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(content)

    print("applied_%d" % len(staged))


if __name__ == "__main__":
    main()
