"""Offline controls for the uncredited glob/logging source-and-evidence stage."""
import copy
import json
import unittest
from pathlib import Path

import raw_control
import scorecard

PIN = "8571ca2dbecabf8d7863c369dc776611fd57d8ad"
SNAPSHOT = scorecard.HERE / "revisions" / PIN


def evidence(family):
    directory = SNAPSHOT / "evidence" / family / "strict"
    binding = json.loads((directory / "binding.json").read_text())["cohorts"]["finite-native"]
    run = json.loads((directory / "run.json").read_text())
    return directory, binding, run


def selected_trace(directory, tier):
    return SNAPSHOT / 'evidence/run-only' / directory.parent.name / tier / 'trace.raw'


def withdrawal(directory, binding, run, tier, trace=None):
    meta = run["runs"][tier]
    tree = str(Path(run["runs"][tier]["environment"]["PYTHONPATH"]) / "test")
    leaf = str(Path(tree) / Path(run["source"]["path"]).name)
    raw_control.trace_withdraw(
        scorecard, trace or selected_trace(directory, tier).read_text(), meta["argv"][0],
        binding["probe"], tree, tree + ".raw-control-stash", leaf, meta["cwd"],
        scorecard.require, binding["withdraw_sources"], strict_payloads=True,
    )


class StagedSourceControls(unittest.TestCase):
    def test_real_direct_and_withdrawal_traces(self):
        for family in ("glob", "logging"):
            with self.subTest(family=family):
                directory, binding, run = evidence(family)
                command = json.loads((directory / "direct.command.json").read_text())["command"]
                raw_control.trace_direct_gate((directory / "direct.trace").read_text(), command,
                                              binding["probe"], (directory / "direct.stderr").read_text(),
                                              scorecard.require)
                for tier in ("O0", "O3"):
                    withdrawal(directory, binding, run, tier)

    def test_direct_gate_requires_raw_post_build_no_image_check(self):
        directory, binding, _ = evidence("glob")
        command = json.loads((directory / "direct.command.json").read_text())["command"]
        image = json.dumps(command[command.index("-o") + 1])
        lines = (directory / "direct.trace").read_text().splitlines()
        checks = [i for i, line in enumerate(lines) if "newfstatat(" in line and image in line]
        self.assertEqual(len(checks), 2)
        del lines[checks[-1]]
        with self.assertRaisesRegex(ValueError, "missing raw trace operation"):
            raw_control.trace_direct_gate("\n".join(lines), command, binding["probe"],
                                          (directory / "direct.stderr").read_text(), scorecard.require)

    def test_native_open_of_hidden_launcher_stash_rejects_credit(self):
        directory, binding, run = evidence("glob")
        lines = selected_trace(directory, "O0").read_text().splitlines()
        target = raw_control.execs(lines, scorecard.require)[1]
        index = next(i for i in range(target[0] + 1, len(lines))
                     if lines[i].startswith(target[1] + " ") and raw_control.OPEN.match(lines[i])
                     and '"/etc/ld.so.cache"' in lines[i])
        lines[index] = lines[index].replace('"/etc/ld.so.cache"',
                                            json.dumps(binding["withdraw_sources"][0] + ".raw-control-stash"))
        with self.assertRaisesRegex(ValueError, "native opened"):
            withdrawal(directory, binding, run, "O0", "\n".join(lines))

    def test_logging_source_archive_is_bound_to_exact_original(self):
        directory, binding, run = evidence("logging")
        archive = directory / "stock.source.zip"
        capture = json.loads((directory / "stock.capture.json").read_text())
        raw_control.validate_stock_source(archive, binding["stock_source_member"], run["source"],
                                          capture, scorecard.require)
        changed = copy.deepcopy(run["source"])
        changed["sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "differs from the pinned source"):
            raw_control.validate_stock_source(archive, binding["stock_source_member"], changed,
                                              capture, scorecard.require)


if __name__ == "__main__":
    unittest.main()
