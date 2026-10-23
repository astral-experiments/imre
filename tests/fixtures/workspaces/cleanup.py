import subprocess
from pathlib import Path
from tempfile import TemporaryDirectory


with TemporaryDirectory(dir="scratch") as name:
    root = Path(name)
    Path("readonly-tree").replace(root / "readonly")
    Path("nested-link").replace(root / "outside-link")
    Path("dangling-link").replace(root / "broken-link")
print(root.exists())
print(Path("outside/keep.txt").read_text(encoding="utf-8"))

with TemporaryDirectory(dir="scratch") as readonly:
    child = Path(readonly) / "locked.txt"
    child.write_text("read-only root", encoding="utf-8")
    subprocess.run(
        ["chmod", "000", str(child), readonly],
        capture_output=True, encoding="utf-8", check=True,
    )
print(Path(readonly).exists())

with TemporaryDirectory(dir="scratch") as missing:
    Path(missing).rmdir()
print(Path(missing).exists())

with TemporaryDirectory(dir="scratch") as moved:
    (Path(moved) / "retained.txt").write_text("moved directory survives", encoding="utf-8")
    Path(moved).replace("relocated")
print(Path(moved).exists())
print(Path("relocated/retained.txt").read_text(encoding="utf-8"))

replaced = ""
try:
    with TemporaryDirectory(dir="scratch") as replaced:
        Path(replaced).rmdir()
        Path("root-symlink").replace(replaced)
except NotADirectoryError:
    print("wrong root-symlink category")
except OSError:
    print("root symlink rejected")
print(Path(replaced).is_dir())
print(Path("outside/keep.txt").read_text(encoding="utf-8"))
Path(replaced).unlink()
print(len(list(Path("scratch").iterdir())))
