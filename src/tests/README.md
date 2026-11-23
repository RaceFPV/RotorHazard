# RotorHazard test suites

Two suites live here.  Run them **separately** -- the in-process suite
monkey-patches the interpreter with gevent, which must not be mixed with the
protocol suite's real network client:

1. In-process unit tests (fast, no server process):

       python -m unittest discover -s src/tests

2. Protocol tests (characterization suite; speaks HTTP + Socket.IO to a
   running server -- spawns a Python server with mock nodes by default, or
   connects to any server given via `RH_TEST_URL`, e.g. a Rust build):

       python -m unittest discover -s src/tests/protocol

Environment variables for the protocol suite:
- `RH_TEST_URL` -- connect to an already-running server instead of spawning one
- `RH_KEEP_SERVER=1` -- leave the spawned server running after the tests
