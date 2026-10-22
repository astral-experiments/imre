from pathlib import Path
from tempfile import TemporaryDirectory


def returned_name() -> str:
    with TemporaryDirectory(dir="scratch") as name:
        (Path(name) / "data.txt").write_text("return", encoding="utf-8")
        return name


def returned_paths() -> list[Path]:
    with TemporaryDirectory(dir="scratch") as name:
        root = Path(name)
        (root / "child").mkdir()
        return [root, root / "child"]


def finalized() -> str:
    with TemporaryDirectory(dir="scratch") as name:
        try:
            return name
        finally:
            print(Path(name).exists())


print(Path(returned_name()).exists())
for path in returned_paths():
    print(path.exists())
print(Path(finalized()).exists())

saved: list[str] = []
name = ""
stream = open("sentinel.txt", "r", encoding="utf-8")
stream.close()
for index in range(96):
    with TemporaryDirectory(dir="scratch") as name:
        saved.append(name)
        nested = Path(name) / "one" / "two"
        nested.mkdir(parents=True)
        with open(nested / "data.txt", "w", encoding="utf-8") as stream:
            stream.write("owned buffer λ")
            if index % 3 == 0:
                continue
            if index == 95:
                break
print(len(saved))
print(all(not Path(name).exists() for name in saved))
print(stream.closed)

counter = 0
while counter < 5:
    counter += 1
    with TemporaryDirectory(dir="scratch") as name:
        if counter < 3:
            continue
        break
print(counter)
print(Path(name).exists())

failed_name = ""
try:
    with TemporaryDirectory(dir="scratch") as failed_name:
        try:
            raise ValueError("body")
        finally:
            print(Path(failed_name).exists())
except ValueError:
    print("body error survives cleanup")
print(Path(failed_name).exists())
print(len(list(Path("scratch").iterdir())))
