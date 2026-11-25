#!/usr/bin/env python3
"""Tests for llm_call.py: request shaping and the no-key guard.

Run: python3 rust/tools/test_llm_call.py
No network calls are made.
"""

import importlib.util
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
MODULE_PATH = os.path.join(HERE, "llm_call.py")

spec = importlib.util.spec_from_file_location("llm_call", MODULE_PATH)
llm_call = importlib.util.module_from_spec(spec)
spec.loader.exec_module(llm_call)

failures = []


def check(name, got, want):
    ok = got == want
    print(("PASS  " if ok else "FAIL  ") + name + " -> " + repr(got))
    if not ok:
        failures.append((name, got, want))


def main():
    # Style inference: OpenRouter is OpenAI-shaped, Anthropic is not.
    check("openrouter url -> openai style",
          llm_call.resolve_style("https://openrouter.ai/api/v1/chat/completions"), "openai")
    check("anthropic url -> anthropic style",
          llm_call.resolve_style("https://api.anthropic.com/v1/messages"), "anthropic")
    check("explicit style wins",
          llm_call.resolve_style("https://gateway.local/v1/chat", "anthropic"), "anthropic")

    headers = llm_call.build_headers("openai", "tok123")
    check("openai sends bearer auth", headers.get("Authorization"), "Bearer tok123")
    check("openai omits x-api-key", headers.get("x-api-key"), None)

    headers = llm_call.build_headers("anthropic", "tok123")
    check("anthropic sends x-api-key", headers.get("x-api-key"), "tok123")
    check("anthropic sends version header", headers.get("anthropic-version"),
          llm_call.ANTHROPIC_VERSION)

    body = llm_call.build_payload("openai", "deepseek/deepseek-v4.1-flash-1017", "hello", 64000)
    check("model forwarded", body["model"], "deepseek/deepseek-v4.1-flash-1017")
    check("max_tokens is an int", body["max_tokens"], 64000)
    check("single user message", body["messages"], [{"role": "user", "content": "hello"}])
    check("reasoning_effort omitted unless configured", "reasoning_effort" in body, False)

    body = llm_call.build_payload("openai", "deepseek/deepseek-v4.1-flash-1017", "hello",
                                  64000, reasoning_effort="high")
    check("reasoning_effort forwarded when set", body.get("reasoning_effort"), "high")

    # Defaults documented in the module match the intended target.
    check("default model", llm_call.DEFAULT_MODEL, "deepseek/deepseek-v4.1-flash-1017")
    check("default base url", llm_call.DEFAULT_BASE_URL,
          "https://openrouter.ai/api/v1/chat/completions")

    # No key: prints 000 and exits non-zero, without attempting a request.
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as handle:
        handle.write("prompt")
        prompt_path = handle.name
    env = {k: v for k, v in os.environ.items() if k != "LLM_API_KEY"}
    proc = subprocess.run([sys.executable, MODULE_PATH, prompt_path,
                           prompt_path + ".out"],
                          capture_output=True, text=True, env=env)
    check("missing key prints 000", proc.stdout.strip(), "000")
    check("missing key exits non-zero", proc.returncode != 0, True)
    os.unlink(prompt_path)

    print()
    if failures:
        print("%d FAILURES" % len(failures))
        return 1
    print("all llm_call checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
