#!/usr/bin/env python3
"""Open (or find) the pull request for an upstream-sync branch.

Usage: open_sync_pr.py --branch upstream-sync/abc1234 --title "..."

Needs GITHUB_TOKEN with pull-requests: write, and GITHUB_REPOSITORY (set by
Actions) unless --repo is given. Idempotent: if a PR for the branch already
exists, its URL is printed and the exit status is still 0, so a re-run of the
sync does not fail on a duplicate.
"""

import argparse
import json
import os
import sys
import urllib.error
import urllib.request

API = "https://api.github.com"

BODY = """Automated port of upstream changes into the Rust tree.

What this branch contains:
- a merge of `RotorHazard/RotorHazard` `main` into `rust`
- the LLM-ported Rust edits for that diff

Gates that passed before this PR was opened:
- `cargo test --release` (exit status)
- the Python protocol suite against the built release binary

Review the ported hunks. If it is wrong, close this PR and the branch; nothing
has landed on `rust`.
"""


def request(method, path, token, payload=None):
    data = json.dumps(payload).encode() if payload is not None else None
    req = urllib.request.Request(
        API + path,
        data=data,
        method=method,
        headers={
            "Authorization": "Bearer " + token,
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "Content-Type": "application/json",
            "User-Agent": "rotorhazard-upstream-sync",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            return resp.status, json.load(resp)
    except urllib.error.HTTPError as exc:
        body = exc.read()
        try:
            return exc.code, json.loads(body)
        except Exception:  # noqa: BLE001 - diagnostics
            return exc.code, {"message": body.decode("utf-8", "replace")}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--branch", required=True)
    ap.add_argument("--title", required=True)
    ap.add_argument("--base", default="rust")
    ap.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""))
    args = ap.parse_args()

    token = (os.environ.get("GITHUB_TOKEN") or "").strip()
    if not token:
        print("no GITHUB_TOKEN; cannot open a pull request")
        return 1
    if "/" not in args.repo:
        print("no GITHUB_REPOSITORY (owner/name); cannot open a pull request")
        return 1

    status, payload = request("POST", "/repos/%s/pulls" % args.repo, token, {
        "title": args.title,
        "head": args.branch,
        "base": args.base,
        "body": BODY,
    })

    if status in (200, 201):
        print("opened PR: %s" % payload.get("html_url"))
        return 0

    # 422 = already open for this head/base pair; report it, do not fail.
    errors = payload.get("errors") or []
    already = any("already exists" in str(e.get("message", "")) for e in errors
                  if isinstance(e, dict))
    if status == 422 and already:
        found_status, found = request(
            "GET", "/repos/%s/pulls?head=%s:%s&state=open"
            % (args.repo, args.repo.split("/")[0], args.branch), token)
        if found_status == 200 and found:
            print("PR already open: %s" % found[0].get("html_url"))
            return 0

    print("could not open PR (HTTP %s): %s" % (status, payload.get("message")))
    for err in errors:
        print("  - %s" % err)
    return 1


if __name__ == "__main__":
    sys.exit(main())
