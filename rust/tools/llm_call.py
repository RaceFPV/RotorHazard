#!/usr/bin/env python3
"""Call the configured LLM provider and write its raw JSON response.

Usage: llm_call.py <prompt_file> <out_file>

Configuration (environment):
  LLM_API_KEY      required; the bearer token / x-api-key credential
  LLM_BASE_URL     default https://openrouter.ai/api/v1/chat/completions
  LLM_MODEL        default deepseek/deepseek-v4.1-flash-1017
  LLM_MAX_TOKENS   default 64000
  LLM_REASONING_EFFORT
                   optional; forwarded as `reasoning_effort` when set.
                   deepseek-v4.1-flash-1017 defaults to "high" already and
                   accepts max|high|low|none. Left unset = provider default.
  LLM_STYLE        "openai" or "anthropic"; inferred from the URL when unset
  LLM_TIMEOUT      default 900 seconds
  LLM_RETRIES      default 3 attempts (network errors, 429 and 5xx)

Prints the HTTP status code on stdout (000 if the request never completed) and
exits non-zero unless a 2xx body was written to <out_file>. Error detail goes
to stderr, so the caller can log it without corrupting the status.

Provider shapes: OpenRouter and Anthropic accept the same request body
({model, max_tokens, messages}); they differ in URL and authentication headers.
Response parsing differences live in validate_port.py, which reads both shapes.
"""

import json
import os
import sys
import time
import urllib.error
import urllib.request

DEFAULT_BASE_URL = "https://openrouter.ai/api/v1/chat/completions"
DEFAULT_MODEL = "deepseek/deepseek-v4.1-flash-1017"
DEFAULT_MAX_TOKENS = 64000
ANTHROPIC_VERSION = "2023-06-01"


def env_or(environ, name, default):
    """Environment value with empty/whitespace treated as absent.

    GitHub Actions passes `${{ vars.X }}` as an empty string when the variable
    is not defined, so a plain dict.get(name, default) would hand back "" and
    silently override the default.
    """
    value = (environ.get(name) or "").strip()
    return value or default


def resolve_config(environ):
    """All tunables in one place, so they can be tested without the network."""
    base_url = env_or(environ, "LLM_BASE_URL", DEFAULT_BASE_URL)
    return {
        "api_key": (environ.get("LLM_API_KEY") or "").strip(),
        "base_url": base_url,
        "model": env_or(environ, "LLM_MODEL", DEFAULT_MODEL),
        "max_tokens": int(env_or(environ, "LLM_MAX_TOKENS", str(DEFAULT_MAX_TOKENS))),
        "timeout": int(env_or(environ, "LLM_TIMEOUT", "900")),
        "attempts": max(1, int(env_or(environ, "LLM_RETRIES", "3"))),
        "style": resolve_style(base_url, env_or(environ, "LLM_STYLE", "")),
        "reasoning_effort": env_or(environ, "LLM_REASONING_EFFORT", "") or None,
    }


def resolve_style(base_url, explicit=None):
    if explicit:
        style = explicit.strip().lower()
        if style not in ("openai", "anthropic"):
            raise SystemExit("LLM_STYLE must be 'openai' or 'anthropic', got %r" % explicit)
        return style
    return "anthropic" if "anthropic" in base_url.lower() else "openai"


def build_payload(style, model, prompt, max_tokens, reasoning_effort=None):
    """Both provider shapes accept this body today."""
    payload = {
        "model": model,
        "max_tokens": int(max_tokens),
        "messages": [{"role": "user", "content": prompt}],
    }
    if reasoning_effort:
        # Only sent when explicitly configured: models that do not understand
        # the field would reject the request, and the provider default is
        # already "high" for the model this pipeline targets.
        payload["reasoning_effort"] = reasoning_effort
    return payload


def build_headers(style, api_key):
    headers = {"Content-Type": "application/json"}
    if style == "anthropic":
        headers["x-api-key"] = api_key
        headers["anthropic-version"] = ANTHROPIC_VERSION
    else:
        headers["Authorization"] = "Bearer " + api_key
        # Optional OpenRouter attribution; harmless elsewhere.
        if os.environ.get("LLM_HTTP_REFERER"):
            headers["HTTP-Referer"] = os.environ["LLM_HTTP_REFERER"]
        if os.environ.get("LLM_APP_TITLE"):
            headers["X-Title"] = os.environ["LLM_APP_TITLE"]
    return headers


def post(url, headers, body, timeout):
    request = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers=headers,
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.status, response.read()


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: llm_call.py <prompt_file> <out_file>")

    prompt_path, out_path = sys.argv[1], sys.argv[2]
    config = resolve_config(os.environ)
    if not config["api_key"]:
        print("000")
        sys.exit("LLM_API_KEY is not set")

    with open(prompt_path, encoding="utf-8") as handle:
        prompt = handle.read()

    headers = build_headers(config["style"], config["api_key"])
    body = build_payload(config["style"], config["model"], prompt,
                         config["max_tokens"],
                         reasoning_effort=config["reasoning_effort"])

    status = "000"
    for attempt in range(1, config["attempts"] + 1):
        try:
            status, payload = post(config["base_url"], headers, body, config["timeout"])
            status = str(status)
        except urllib.error.HTTPError as exc:
            status = str(exc.code)
            payload = exc.read() or b""
            # 4xx other than rate limiting will not improve on retry.
            if exc.code != 429 and exc.code < 500:
                break
        except Exception as exc:  # noqa: BLE001 - network layer, any failure retries
            status = "000"
            payload = b""
            sys.stderr.write("llm_call: attempt %d failed: %s\n" % (attempt, exc))

        with open(out_path, "wb") as handle:
            handle.write(payload)

        if status.isdigit() and 200 <= int(status) < 300:
            print(status)
            return 0

        if attempt < config["attempts"]:
            time.sleep(min(30, 5 * attempt))

    print(status)
    sys.stderr.write("llm_call: giving up with status %s\n" % status)
    return 1


if __name__ == "__main__":
    sys.exit(main())
