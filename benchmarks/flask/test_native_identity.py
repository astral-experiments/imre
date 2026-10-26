"""Regression controls for the standalone gate's Linux process trace."""
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from native_identity import audit_trace, require_attached


class TraceTests(unittest.TestCase):
    def test_denied_attachment(self):
        with patch("native_identity.Path.read_text", return_value="Name:\tserver\nTracerPid:\t0\n"):
            with self.assertRaisesRegex(ValueError, "ptrace permissions"):
                require_attached(41, Path("events"), "/usr/bin/strace")

    def audit(self, *events):
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "server"
            binary.touch()
            path = Path(temp) / "events"
            events = [f'41 execve("{binary}", ["server"], 0x1) = 0', *events, "41 +++ killed by SIGTERM +++"]
            path.write_text("\n".join(events) + "\n")
            return audit_trace(path, binary, 41)

    def test_native_workers(self):
        proof = self.audit("41 clone3({flags=CLONE_VM|CLONE_THREAD}, 88) = 42",
                           "41 clone(child_stack=NULL, flags=SIGCHLD) = 43",
                           "42 +++ exited with 0 +++", "43 +++ exited with 0 +++")
        self.assertEqual(len(proof["worker_events"]), 2)
        self.assertEqual(proof["exec_pids"], [41])
        self.assertEqual(proof["libpython_mappings"], 0)

    def test_transient_foreign_exec(self):
        with self.assertRaisesRegex(ValueError, "unverified executable"):
            self.audit("41 fork() = 42", '42 execve("/some/other/program", [], 0x1) = 0',
                       "42 +++ exited with 0 +++")

    def test_transient_libpython(self):
        with self.assertRaisesRegex(ValueError, "mapped libpython"):
            self.audit("41 mmap(NULL, 4096, PROT_READ, MAP_PRIVATE, 3</lib/libpython3.14.so.1.0>, 0) = 0x1000")

    def test_incomplete_worker_lifetime(self):
        with patch("native_identity.time.monotonic", side_effect=[0, 1, 5]):
            with self.assertRaisesRegex(ValueError, "through exit"):
                self.audit("41 fork() = 42")


if __name__ == "__main__":
    unittest.main()
