from pathlib import Path


for name in ["denied", "denied/secret.json", "denied.txt"]:
    path = Path(name)
    print(path.exists())
    print(path.is_file())
    print(path.is_dir())
try:
    print(Path("denied.txt").read_text(encoding="utf-8"))
except PermissionError:
    print("permission read")
try:
    print(Path("denied.txt").write_text("replacement", encoding="utf-8"))
except PermissionError:
    print("permission write")
try:
    print(len(list(Path("denied").iterdir())))
except PermissionError:
    print("permission traversal")
print(len(list(Path("denied").glob("*.json"))))
print(len(list(Path("denied").glob("secret.json"))))
