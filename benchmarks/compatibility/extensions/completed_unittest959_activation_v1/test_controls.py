"""Exercise the shared finite core against the sealed completed-run captures."""

import functools
import json
import pathlib
import re
import sys
import unittest

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[3]
COMPATIBILITY = ROOT / "benchmarks/compatibility"
sys.path.insert(0, str(COMPATIBILITY))

import raw_control
import scorecard
from extensions.completed_unittest959_activation_v1.identity_roles import DirectCapture
from extensions.successor959_v1 import authority as common

REVISION = "95908fb0acadc2a67304a8080e584deaef395746"


class Controls(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        scorecard.select_snapshot(REVISION)
        cls.binding = json.loads((HERE / "binding.json").read_bytes())

    @functools.lru_cache(maxsize=None)
    def data(self, role):
        locator = self.binding["roles"][role]
        archive, separator, member = locator.partition("!")
        if not separator:
            return (ROOT / locator).read_bytes()
        config = json.dumps(self.binding["archives"][archive], sort_keys=True)
        return common.archive_members(scorecard, archive, config)[member]

    def obj(self, role):
        return json.loads(self.data(role))

    def capture(self):
        return DirectCapture(scorecard, self.binding, self.data, self.obj)

    def core(self):
        capture = self.capture()
        return raw_control.finite_core(
            scorecard,
            self.binding["family"],
            self.binding["source"],
            {},
            lambda role: ROOT / self.binding["roles"][role],
            self.data,
            lambda role: self.data(role).decode(),
            capture.object,
            capture.context(),
        )

    def test_real_full_shared_core(self):
        run, inventory, _ = self.core()
        self.assertEqual([len(inventory[tier]) for tier in scorecard.TIERS], [276] * 3)
        self.assertEqual([run["runs"][tier]["exit_code"] for tier in scorecard.TIERS], [0, 1, 1])
        paired = sum(
            all(inventory[tier][key]["outcome"] == "pass" for tier in scorecard.TIERS)
            for key in inventory["stock"]
        )
        self.assertEqual(paired, 273)
        self.assertTrue(
            all(
                record["outcome"] in {"pass", "skip", "error"}
                for tier in inventory.values()
                for record in tier.values()
            )
        )

    def test_native_subtest_skip_is_not_creditable(self):
        stderr = self.data("O0_stderr")
        lines = stderr.splitlines(keepends=True)
        event = b'IMRE_UNITTEST {"event":"subtest_skip","id":"parent","reason":"native-only"}\n'
        mutated = b"".join(lines[:-1]) + event + lines[-1]
        trace = self.data("O0_trace").decode()
        pid = raw_control.exec_sequence(trace.splitlines(), scorecard.require, 2)[-1][1]
        summary = next(
            line for line in trace.splitlines()
            if line.startswith(pid + " ") and "write(2<" in line and "summary" in line
        )
        pipe = re.search(r"write\(2(<pipe:\[[0-9]+\]>)", summary)[1]
        write = f"{pid} write(2{pipe}, {json.dumps(event.decode())}, {len(event)}) = {len(event)}"
        rebound = trace.replace(summary, write + "\n" + summary, 1)
        canonical = [(row["id"], row["occurrence"]) for row in self.binding["originals"]]

        self.assertEqual(
            raw_control.traced_output(rebound.splitlines(), pid, 2, scorecard.require), mutated
        )
        with self.assertRaisesRegex(ValueError, "subtest|summary, outcomes"):
            raw_control.completed_unittest(
                mutated, 1, rebound, pid, scorecard.require, stdout=b"", canonical=canonical
            )

    def test_fresh_image_controller_substitution_rejected(self):
        capture = self.capture()
        context = capture.context()
        metadata = capture.object("O0_meta")
        build = capture.object("O0_build_json")

        def substitute_controller(role):
            return self.data("O3_build_controller" if role == "O0_build_controller" else role)

        with self.assertRaisesRegex(ValueError, "positive raw compiler"):
            raw_control.finite_identity_build(
                scorecard, "O0", context, build, metadata, substitute_controller,
                lambda role: substitute_controller(role).decode(),
            )


if __name__ == "__main__":
    unittest.main()
