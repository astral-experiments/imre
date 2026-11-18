"""Rebound trace/receipt attacks against the fixed Ruff-06 source authority."""

import copy
import hashlib
import json
import shutil
import tempfile
import unittest
from pathlib import Path

import policy_gate as g


class PolicyAttacks(unittest.TestCase):
    def test_real_variants(self):
        for t in ("O0", "O3"):
            for v in ("original", "invalid", "withdrawn"):
                g.validate(t + "-" + v)

    def rebind(self, folder, binding, trace, mutate_run=None):
        role = binding["runs"]["O0-original"]
        run = g.data(g.HERE / role["run"])
        tp = folder / "attack.trace"
        tp.write_text(trace)
        run["trace_file"] = tp.name
        run["trace_sha256"] = g.sha(tp)
        if mutate_run:
            mutate_run(run)
        rp = folder / "attack.json"
        rp.write_text(json.dumps(run))
        role.update(
            trace=str(tp.relative_to(g.HERE)),
            trace_sha256=g.sha(tp),
            run=str(rp.relative_to(g.HERE)),
            run_sha256=g.sha(rp),
        )
        return binding

    def test_rehashed_attacks(self):
        base = g.data(g.HERE / "binding.json")
        r = g.data(g.HERE / base["runs"]["O0-original"]["run"])
        text = (g.HERE / base["runs"]["O0-original"]["trace"]).read_text()
        lines = text.splitlines()
        m = next(
            g.raw_control.EXEC.fullmatch(x)
            for x in lines
            if g.raw_control.EXEC.fullmatch(x)
        )
        pid = m[1]
        cwd = r["cwd"]
        at = f"AT_FDCWD<{cwd}>"
        entry = f"{cwd}/{g.ENTRY}"
        ro = "O_RDONLY|O_CLOEXEC"
        insert = lambda s: "\n".join([lines[0], s, *lines[1:]]) + "\n"
        cases = {
            "extra-py": f'{pid} openat({at}, "/tmp/extra.py", {ro}) = 40</tmp/extra.py>',
            "unknown-suffix": f'{pid} openat({at}, "/tmp/payload.blob", {ro}) = 40</tmp/payload.blob>',
            "write-flag": f'{pid} openat({at}, "{entry}", O_RDWR|O_CLOEXEC) = 40<{entry}>',
            "path-alias": f'{pid} openat({at}, "{cwd}/scripts/../scripts/check-scripts.py", {ro}) = 40<{entry}>',
            "dirfd-alias": f'{pid} openat(39<{cwd}/scripts>, "check-scripts.py", {ro}) = 40<{entry}>',
            "unknown-dirfd": f'{pid} openat(39, "check-scripts.py", {ro}) = 40<{entry}>',
            "fd-alias": f'{pid} openat({at}, "/proc/self/fd/39", {ro}) = 40<{entry}>',
            "openat2": f'{pid} openat2({at}, "{entry}", {{flags={ro}, resolve=0}}, 24) = 40<{entry}>',
            "creat": f'{pid} creat("{entry}", 0600) = 40<{entry}>',
            "exec": f'{pid} execve("/usr/bin/python3", ["python3"], 0x0) = 0',
        }
        with tempfile.TemporaryDirectory(dir=g.HERE) as d:
            folder = Path(d)
            for name, call in cases.items():
                with self.subTest(name=name), self.assertRaises(ValueError):
                    g.validate(
                        "O0-original",
                        self.rebind(folder, copy.deepcopy(base), insert(call)),
                    )
            with self.assertRaisesRegex(ValueError, "source authority"):
                g.validate(
                    "O0-original",
                    self.rebind(
                        folder,
                        copy.deepcopy(base),
                        text,
                        lambda x: x["before"].__setitem__(g.ENTRY, "0" * 64),
                    ),
                )
            read_line = next(
                x
                for x in lines
                if " read(" in x
                and "/scripts/check-scripts.py>" in x
                and "uv run --script" in x
            )
            corrupted = text.replace(
                read_line, read_line.replace("uv run --script", "xx run --script")
            )
            with self.assertRaisesRegex(
                ValueError, "read bytes differ.*source authority"
            ):
                g.validate(
                    "O0-original", self.rebind(folder, copy.deepcopy(base), corrupted)
                )
            fake = folder / "fake-authority"
            fake.mkdir()
            shutil.copytree(g.HERE / "source", fake / "source")
            shutil.copy2(g.HERE / "policy-original.json", fake / "policy-original.json")
            entry = fake / "source/fixture" / g.ENTRY
            entry.write_bytes(b"# /// script\nchanged\n")
            changed_binding = copy.deepcopy(base)
            changed_binding["runs"]["O0-original"]["run_sha256"] = hashlib.sha256(
                b"rebound receipt"
            ).hexdigest()
            with self.assertRaisesRegex(ValueError, "source authority"):
                g.validate("O0-original", changed_binding, root=fake)
            changed = copy.deepcopy(base)
            changed["runs"]["O0-original"]["policy_variant"] = "invalid-control"
            with self.assertRaisesRegex(ValueError, "variant"):
                g.validate("O0-original", changed)


if __name__ == "__main__":
    unittest.main()
