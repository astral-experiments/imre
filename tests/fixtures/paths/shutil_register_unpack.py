import shutil
from pathlib import Path


def extract(filename: str, extract_dir: str, extra: int, *, filter: str = "default") -> None:
    print(repr((filename, extract_dir, extra, filter)))
    Path("callback-marker").write_text(filename + ":" + str(extra))


def remove_self(filename: str, extract_dir: str) -> None:
    shutil.unregister_unpack_format("replace")
    raise shutil.ReadError("archive callback failed")


def filter_only(filename: str, extract_dir: str, *, filter: str) -> None:
    print(repr((filename, extract_dir, filter)))


def show_entry(name: str, extensions: list[str]) -> None:
    for registered, suffixes, description in shutil.get_unpack_formats():
        if registered == name:
            print(suffixes is extensions)
            print(description)


extensions = [".boo", ".b2"]
extras = [("extra", 1)]
shutil.register_unpack_format("Boo", extensions, extract, extra_args=extras, description="callback")
show_entry("Boo", extensions)
shutil.unpack_archive("stuff.boo", "xx")
print(Path("callback-marker").read_text())
extras.append(("extra", 2))
extensions.append(".alias")
shutil.unpack_archive(Path("stuff.alias"), "", filter="data")
print(Path("callback-marker").read_text())
try:
    shutil.register_unpack_format("Boo2", [".boo"], 1)
except shutil.RegistryError as error:
    print(str(error))
try:
    shutil.register_unpack_format("Boo2", [".free"], 1)
except TypeError as error:
    print(str(error))
shutil.register_unpack_format("Boo", [".replace"], extract)
try:
    shutil.unpack_archive("stuff.boo", ".")
except shutil.ReadError as error:
    print(str(error))
try:
    shutil.unpack_archive("stuff.replace", ".")
except TypeError as error:
    print(str(error))
shutil.unregister_unpack_format("Boo")

options = {"filter": "registered"}
shutil.register_unpack_format("filters", [".filters"], filter_only, options)
shutil.unpack_archive("thing.filters", "out")
options["filter"] = "changed"
shutil.unpack_archive("thing.filters", "out", "filters")
shutil.unpack_archive("thing.filters", "out", filter="explicit")
try:
    shutil.unpack_archive("thing.filters", "out", "filters", filter="explicit")
except TypeError as error:
    print(str(error))
shutil.unregister_unpack_format("filters")

shutil.register_unpack_format("replace", [".remove-self"], remove_self)
try:
    shutil.unpack_archive("thing.remove-self", ".")
except shutil.ReadError as error:
    print(str(error))
try:
    shutil.unregister_unpack_format("replace")
except KeyError as error:
    print(str(error))
Path("callback-marker").unlink()
