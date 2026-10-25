from pathlib import Path


def failed() -> str:
    print("argument failed")
    raise ValueError("do not truncate")


def remove_owner(paths: list[Path]) -> str:
    paths.pop()
    return "receiver survives its final container owner λ🙂"


path = Path("input.txt")
print(path.read_text())
print(path.exists())
print(Path('.').exists(follow_symlinks=False))
print(path.is_file())
print(path.is_dir())
output = Path("output.txt")
print(output.write_text("α\r\nβ\r🙂\0end", encoding="utf-8"))
print(output.read_text(encoding="utf-8"))
print(output.write_text("UTF-8 default λ🙂"))
print(output.read_text())
print(Path("empty.txt").read_text(encoding="utf-8"))
with open(Path("binary.dat"), "wb") as stream:
    print(stream.write(b"\xff\x00\n"))
with open(Path("binary.dat"), "rb") as stream:
    print(stream.read())
with open(Path("via-open.txt"), "w", encoding="utf-8") as text_stream:
    print(text_stream.write("path accepted λ"))
with open(Path("via-open.txt"), "r", encoding="utf-8") as text_stream:
    print(text_stream.read())
try:
    Path("sentinel.txt").write_text(failed(), encoding="utf-8")
except ValueError:
    print(Path("sentinel.txt").read_text(encoding="utf-8"))
paths = [Path("rooted.txt")]
print(paths[0].write_text(remove_owner(paths), encoding="utf-8"))
print(len(paths))
for name in ["tree", "missing", "input.txt/child", "file-link", "directory-link", "dangling-link", "bad\0path"]:
    candidate = Path(name)
    print(name)
    print(candidate.exists())
    print(candidate.is_file())
    print(candidate.is_dir())
    print(
        f"{candidate.exists(follow_symlinks=False)} "
        f"{candidate.is_file(follow_symlinks=False)} "
        f"{candidate.is_dir(follow_symlinks=False)}"
    )
print(Path("file-link").read_text(encoding="utf-8"))
