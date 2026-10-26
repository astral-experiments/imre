from pathlib import Path

cwd = Path(".").absolute()
assert cwd.name == "empty-directory"
cwd.rmdir()

for relative in [".", "child", "../sibling"]:
    try:
        print(str(Path(relative).absolute()))
    except FileNotFoundError as error:
        print((error.errno, error.strerror, repr(error.filename), repr(error.args)))
print(str(Path("/already-absolute").absolute()))
