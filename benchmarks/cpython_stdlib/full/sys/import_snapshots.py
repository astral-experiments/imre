"""Compare side effects of from-imports at their original position in module initialization."""
from pathlib import Path
import subprocess
import sys
import textwrap


def check_import_snapshots(toolchain, work):
    scenarios = {
        "retain": {
            "prime": "import builtins\nbuiltins._ = [1, 2]\n",
            "replace": "import builtins\nbuiltins._ = 'changed'\n",
            "snapshot": "from builtins import _ as held\nimport replace\n",
            "entry": "import prime\nfrom snapshot import held\nimport builtins\nprint(held)\nprint(builtins._)\n",
        },
        "missing": {"entry": "from builtins import _ as held\nprint(held)\n"},
        "none": {
            "clear": "import sys\nsys.stdout = None\n",
            "entry": "import clear\nfrom sys import stdout as saved, __stdout__ as original\n"
                     "original.write(str(saved is None) + '\\n')\n",
        },
        "deleted": {
            "discard": "import sys\ndel sys.stdout\n",
            "entry": "import discard\nfrom sys import stdout as saved\n",
        },
        "caught": {"entry": textwrap.dedent("""\
            import builtins
            import sys

            def report(error: ImportError) -> None:
                print(repr(error.args))
                print(repr(error.msg))
                print(repr(error.name))
                print(repr(error.path))
                print(repr(error.name_from))

            def check() -> None:
                try:
                    from builtins import _ as saved
                    print(repr(saved))
                except ImportError as error:
                    report(error)
                builtins._ = None
                from builtins import _ as restored
                print(restored is None)
                original = sys.__stdout__
                del sys.stdout
                try:
                    from sys import stdout as absent
                    original.write(str(absent is None))
                except ImportError as error:
                    sys.stdout = original
                    report(error)

            check()
        """)},
    }
    for name, files in scenarios.items():
        folder = Path(work) / name
        folder.mkdir()
        for module, source in files.items():
            (folder / f"{module}.py").write_text(source)
        entry = folder / "entry.py"
        binary = toolchain.build(entry.read_bytes(), folder)
        # Explicit module root also works with a host that enables PYTHONSAFEPATH.
        script = "import runpy,sys;sys.path.insert(0,sys.argv[1]);runpy.run_path(sys.argv[2])"
        outputs = [subprocess.run(command, capture_output=True, timeout=15)
                   for command in ([sys.executable, "-c", script, folder, entry], [binary])]
        # Traceback renderers differ. Compare the exception type and full message.
        observations = [(item.returncode, item.stdout, item.stderr.splitlines()[-1:]) for item in outputs]
        expected_status = int(name in {"missing", "deleted"})
        if observations[0] != observations[1] or observations[0][0] != expected_status:
            raise AssertionError((name, observations))
