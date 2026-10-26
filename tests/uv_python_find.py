"""Discover the built Imre executable using the probe embedded in stock uv."""

import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    args = parser.parse_args()
    compiler = args.compiler.resolve(strict=True)
    target = json.loads(subprocess.check_output([compiler, "build-target"]))
    with tempfile.TemporaryDirectory(prefix="imre-uv-find-") as tmp:
        root = Path(tmp)
        env = {
            **{k: v for k, v in os.environ.items() if not k.startswith("UV_")},
            "PATH": os.pathsep.join((str(compiler.parent), os.environ["PATH"])),
            "PYTHONPATH": str(root),
            "IMRE_CACHE_DIR": str(root / "native cache"),
            "UV_CACHE_DIR": str(root / "discovery cache"),
            "UV_OFFLINE": "1",
            "UV_NO_CONFIG": "1",
            "UV_PYTHON_DOWNLOADS": "never",
        }
        env.pop("VIRTUAL_ENV", None)
        (root / "sitecustomize.py").write_text("raise SystemExit('ignored under -I')\n")

        def find(*options):
            return subprocess.check_output(
                ["uv", "python", "find", *options, "imre"],
                cwd=root,
                env=env,
                text=True,
            ).strip()

        assert Path(find()).samefile(compiler)
        version = find("--show-version").split(".")
        assert version[:2] == target["python_version"].split("."), version

        app = root / "app.py"
        app.write_text(
            "import sys\nprint(f'{sys.implementation.name} {sys.argv[1]}')\n"
        )
        output = subprocess.check_output(
            ["uv", "run", "--python", "imre", "app.py", "hello"],
            cwd=root,
            env=env,
        )
        assert output == b"imre hello\n", output


if __name__ == "__main__":
    main()
