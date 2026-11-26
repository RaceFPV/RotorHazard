#!/usr/bin/env python3
"""Validate and apply the Rust edits in an upstream-sync LLM response.

Usage: validate_port.py <response.json> <sanitized_diff_bytes>

Always exits 0 and prints exactly one result token; upstream-sync.sh keys off
the token:

  applied_<n>              n files validated and written
  no_edits                 response held no JSON array
  parse_error              response or JSON array did not parse
  truncated_response       the model hit its output cap; nothing was applied
  size_exceeded            output far larger than the Python diff
  bad_path:<p>             edit outside the Rust tree
  empty_content:<p>        full-file edit blanked a file
  bad_edits:<p>            element had neither new_content nor a usable edits[]
  anchor_missing:<p>       an "old" block does not occur in the file
  anchor_ambiguous:<p>     an "old" block occurs more than once
  anchor_too_short:<p>     an "old" block is too small to be unique
  anchor_malformed:<p>     an edits[] entry is not {old, new} strings
  truncated:<p>            result lost most of an existing file
  blocked:<kind>:<rule>:<p> an added line violated a rule

Two element shapes are accepted, and may be mixed in one response:

  {"path": P, "new_content": "<entire file>"}
  {"path": P, "edits": [{"old": "<exact existing text>", "new": "<replacement>"}]}

Anchored edits exist because a full-file re-emit is only bounded by the output
cap: `socket.rs` is ~3,000 lines / ~39k output tokens, so a five-line upstream
change costs a whole-file regeneration and any cosmetic drift the model
introduces lands in the commit. Anchors keep the payload proportional to the
change and the diff equal to the intent.

Rules run on ADDED LINES ONLY -- for a full-file element by diffing against the
file on disk, for an anchored element by diffing each old/new block pair.
Scanning whole file bodies rejects files that legitimately contain a banned
word: auth modules say `PASSWORD`, asset loaders say `include_str!`, the Java
bridge says `std::process::Command`.

Reads both provider response shapes: Anthropic (`content[].text`) and
OpenAI/OpenRouter (`choices[].message.content`).
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

# A result shorter than this fraction of the original almost always means the
# response was cut off, which silently mangles big modules.
TRUNCATION_RATIO = 0.5
TRUNCATION_MIN_BYTES = 400

# Shortest block accepted as an anchor; anything smaller cannot be unique.
MIN_ANCHOR_CHARS = 12

# finish/stop reasons that mean the model ran out of output budget.
CAP_REASONS = {"length", "max_tokens"}


class Rejected(Exception):
    """Validation failure carrying the result token to print."""

    def __init__(self, token):
        super().__init__(token)
        self.token = token


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


def _block_text(block):
    if isinstance(block, str):
        return block
    if isinstance(block, dict) and block.get("type") in (None, "text"):
        return block.get("text") or block.get("content") or ""
    return ""


def response_text(response):
    """Extract assistant text from either provider's response shape."""
    parts = []

    # OpenAI / OpenRouter: choices[].message.content (string or block list)
    for choice in response.get("choices") or []:
        message = choice.get("message") or {}
        content = message.get("content")
        if isinstance(content, str):
            parts.append(content)
        elif isinstance(content, list):
            parts.extend(_block_text(block) for block in content)

    # Anthropic: content[].text
    for block in response.get("content") or []:
        parts.append(_block_text(block))

    return "".join(parts)


def response_hit_cap(response):
    """True when the provider reports the response was cut at the output cap."""
    for choice in response.get("choices") or []:
        if str(choice.get("finish_reason") or "").lower() in CAP_REASONS:
            return True
    return str(response.get("stop_reason") or "").lower() in CAP_REASONS


def _replace_linewise(text, old, new, rstrip_only):
    """Replace the unique line window matching `old`, tolerating whitespace.

    Returns (new_text, matched_original) or None. Matching on rstripped lines
    first, then fully stripped lines, keeps indentation drift from failing a
    port while the uniqueness requirement keeps it safe.
    """
    text_lines = text.splitlines()
    old_lines = old.splitlines()
    if not old_lines or len(old_lines) > len(text_lines):
        return None

    def norm(line):
        return line.rstrip() if rstrip_only else line.strip()

    wanted = [norm(line) for line in old_lines]
    hits = []
    span = len(wanted)
    for start in range(len(text_lines) - span + 1):
        window = [norm(line) for line in text_lines[start:start + span]]
        if window == wanted:
            hits.append(start)
            if len(hits) > 1:
                return None
    if not hits:
        return None

    start = hits[0]
    matched = "\n".join(text_lines[start:start + span])
    new_lines = new.splitlines()
    result = text_lines[:start] + new_lines + text_lines[start + span:]
    return "\n".join(result), matched


def apply_anchors(path, base_text, anchors):
    """Apply [{old,new}] to base_text. Returns (text, added_lines)."""
    if not isinstance(anchors, list) or not anchors:
        raise Rejected("bad_edits:" + path)

    text = base_text
    added = []
    for anchor in anchors:
        if not isinstance(anchor, dict):
            raise Rejected("anchor_malformed:" + path)
        old = anchor.get("old")
        new = anchor.get("new")
        if not isinstance(old, str) or not isinstance(new, str):
            raise Rejected("anchor_malformed:" + path)
        if len(old.strip()) < MIN_ANCHOR_CHARS:
            raise Rejected("anchor_too_short:" + path)

        occurrences = text.count(old)
        if occurrences > 1:
            raise Rejected("anchor_ambiguous:" + path)
        if occurrences == 1:
            text = text.replace(old, new, 1)
            added.extend(added_lines(old, new))
            continue

        # Exact match failed. A model quoting from ~200k tokens of context
        # often loses trailing whitespace or re-indents a block; rejecting on
        # that throws away an otherwise correct port. Retry line-wise: trailing
        # whitespace first, then indentation, each requiring a unique window.
        replaced = _replace_linewise(text, old, new, rstrip_only=True)
        if replaced is None:
            replaced = _replace_linewise(text, old, new, rstrip_only=False)
        if replaced is None:
            raise Rejected("anchor_missing:" + path)
        text, matched = replaced
        added.extend(added_lines(matched, new))

    return text, added


def scan(path, lines):
    for kind, ruleset in RULESETS:
        for pattern, rule in ruleset:
            for line in lines:
                if re.search(pattern, line):
                    raise Rejected("blocked:%s:%s:%s" % (kind, rule, path))


def main():
    response_path = sys.argv[1]
    sanitized_size = int(sys.argv[2] or 0)

    try:
        with open(response_path, encoding="utf-8", errors="replace") as handle:
            response = json.load(handle)
    except Exception:
        print("parse_error")
        return
    if not isinstance(response, dict):
        print("parse_error")
        return

    # Check the cap first: a cut-off array is indistinguishable from a
    # malformed one, and the two need different words in the run log.
    if response_hit_cap(response):
        print("truncated_response")
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
            for anchor in edit.get("edits") or []:
                if isinstance(anchor, dict):
                    total_size += len(anchor.get("new") or "")
    if sanitized_size > 0 and total_size > sanitized_size * 20:
        print("size_exceeded")
        return

    # Base content per path, so two elements for one file compose in order.
    current = {}
    staged = []
    try:
        for edit in edits:
            if not isinstance(edit, dict):
                raise Rejected("bad_edits:<element>")
            path = (edit.get("path") or "").replace("\\", "/")

            if not path.startswith(ALLOWED_PREFIXES) or ".." in path:
                raise Rejected("bad_path:" + path)

            if path in current:
                base = current[path]
            elif os.path.exists(path):
                with open(path, encoding="utf-8", errors="replace") as handle:
                    base = handle.read()
            else:
                base = ""

            if "new_content" in edit:
                result = edit.get("new_content") or ""
                if not result.strip():
                    raise Rejected("empty_content:" + path)
                added = added_lines(base, result)
            elif "edits" in edit:
                result, added = apply_anchors(path, base, edit.get("edits"))
            else:
                raise Rejected("bad_edits:" + path)

            if (len(base) > TRUNCATION_MIN_BYTES
                    and len(result) < len(base) * TRUNCATION_RATIO):
                raise Rejected("truncated:" + path)

            scan(path, added)
            current[path] = result
            staged.append(path)
    except Rejected as rejection:
        print(rejection.token)
        return

    for path in dict.fromkeys(staged):
        parent = os.path.dirname(path)
        if parent:
            os.makedirs(parent, exist_ok=True)
        with open(path, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(current[path])

    print("applied_%d" % len(set(staged)))


if __name__ == "__main__":
    main()
