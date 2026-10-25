import shutil
from pathlib import Path


def pack_names() -> list[str]:
    names: list[str] = []
    for entry in shutil.get_archive_formats():
        names.append(entry[0])
        if entry[0] == "zip":
            print(entry[1])
    return names


def zip_extensions() -> list[str]:
    for entry in shutil.get_unpack_formats():
        if entry[0] == "zip":
            return entry[1]
    raise AssertionError("no zip unpacker")


names = pack_names()
print(repr(names) == repr(sorted(names)))
listed = shutil.get_archive_formats()
while listed:
    listed.pop()
print("zip" in pack_names())
Path("empty-directory/payload").write_bytes(b"pack and unpack")
archive = shutil.make_archive("registry-archive", "zip", root_dir="empty-directory")
alias = Path("packet.registry-test")
alias.write_bytes(Path(archive).read_bytes())
try:
    shutil.unpack_archive(alias, "unpacked")
except shutil.ReadError as error:
    print(str(error))

extensions = zip_extensions()
print(extensions is zip_extensions())
extensions.append(".registry-test")
shutil.unregister_archive_format(name="zip")
print("zip" in pack_names())
try:
    shutil.make_archive("registry-archive", "zip", dry_run=True)
except ValueError as error:
    print(str(error))
shutil.unpack_archive(alias, "unpacked")
print(repr(Path("unpacked/payload").read_bytes()))

shutil.unregister_unpack_format("zip")
try:
    shutil.unpack_archive(alias, "unpacked")
except shutil.ReadError as error:
    print(str(error))
try:
    shutil.unpack_archive(alias, "unpacked", "zip")
except ValueError as error:
    print(str(error))
try:
    shutil.unregister_unpack_format("zip")
except KeyError as error:
    print(str(error))
print(repr(extensions))
Path(archive).unlink()
alias.unlink()
