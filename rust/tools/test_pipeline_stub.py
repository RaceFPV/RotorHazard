#!/usr/bin/env python3
"""End-to-end check: stub provider -> llm_call.py -> validate_port.py.

Run: python3 rust/tools/test_pipeline_stub.py

Starts a local HTTP server that answers in OpenRouter's shape, points
llm_call.py at it with a dummy key, then feeds the saved response to the
validator inside a scratch tree. Proves request shaping, transport and response
parsing fit together -- and that a successful round trip leaves the file
byte-identical -- without spending a token or needing a real key.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import importlib.util
from http.server import BaseHTTPRequestHandler, HTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
captured = {}

# Read the module's own default rather than duplicating the string here.
_spec = importlib.util.spec_from_file_location("llm_call", os.path.join(HERE, "llm_call.py"))
_llm_call = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_llm_call)
llm_default_model = _llm_call.DEFAULT_MODEL


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        captured["body"] = json.loads(self.rfile.read(length))
        captured["headers"] = dict(self.headers)

        new_content = captured["body"]["messages"][0]["content"]
        payload = {
            "id": "gen-stub",
            "model": captured["body"]["model"],
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": json.dumps(
                    [{"path": "rust/server/src/db.rs", "new_content": new_content}])},
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 10},
        }
        body = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


def main():
    server = HTTPServer(("127.0.0.1", 0), Handler)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()

    sandbox = tempfile.mkdtemp(prefix="e2e-")
    try:
        os.makedirs(os.path.join(sandbox, "rust/server/src"))
        target = os.path.join(sandbox, "rust/server/src/db.rs")
        shutil.copy(os.path.join(REPO, "rust/server/src/db.rs"), target)
        original = open(target, encoding="utf-8").read()

        # The prompt is the file body the stub echoes back, so a successful
        # round trip must produce an identical file and pass validation.
        prompt_path = os.path.join(sandbox, "prompt.txt")
        with open(prompt_path, "w", encoding="utf-8") as handle:
            handle.write(original)

        out_path = os.path.join(sandbox, "response.json")
        env = dict(os.environ)
        env.update({
            "LLM_API_KEY": "dummy-token",
            "LLM_BASE_URL": "http://127.0.0.1:%d/api/v1/chat/completions" % port,
            "LLM_MODEL": "deepseek/deepseek-v4.1-flash-1017",
            "LLM_MAX_TOKENS": "64000",
            "LLM_RETRIES": "1",
            "LLM_TIMEOUT": "30",
        })
        proc = subprocess.run([sys.executable, os.path.join(HERE, "llm_call.py"),
                               prompt_path, out_path],
                              capture_output=True, text=True, env=env)
        print("llm_call exit:", proc.returncode, "| status:", proc.stdout.strip())
        if proc.stderr.strip():
            print("llm_call stderr:", proc.stderr.strip())

        headers = captured.get("headers", {})
        body = captured.get("body", {})
        print("request model:", body.get("model"))
        print("request max_tokens:", body.get("max_tokens"))
        print("auth header ok:", headers.get("Authorization") == "Bearer dummy-token")
        print("content-type ok:", headers.get("Content-Type") == "application/json")

        # Actions passes an undefined vars.X as an empty string. That must fall
        # back to the default model rather than send model: "".
        env_blank = dict(env)
        env_blank["LLM_MODEL"] = ""
        env_blank["LLM_MAX_TOKENS"] = ""
        proc_blank = subprocess.run([sys.executable, os.path.join(HERE, "llm_call.py"),
                                     prompt_path, out_path],
                                    capture_output=True, text=True, env=env_blank)
        blank_body = captured.get("body", {})
        print("blank vars -> status:", proc_blank.stdout.strip(),
              "| model:", blank_body.get("model"),
              "| max_tokens:", blank_body.get("max_tokens"))

        result = subprocess.run([sys.executable, os.path.join(HERE, "validate_port.py"),
                                 out_path, "100000"],
                                cwd=sandbox, capture_output=True, text=True)
        print("validator:", result.stdout.strip())

        after = open(target, encoding="utf-8").read()
        print("file unchanged after round trip:", after == original)

        ok = (proc.returncode == 0
              and result.stdout.strip() == "applied_1"
              and after == original
              and body.get("max_tokens") == 64000
              and headers.get("Authorization") == "Bearer dummy-token"
              and blank_body.get("model") == llm_default_model
              and blank_body.get("max_tokens") == 64000)
    finally:
        shutil.rmtree(sandbox, ignore_errors=True)
        server.shutdown()

    print("\nE2E:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
