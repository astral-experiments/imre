from facade import Location, Workspace
import pathlib as paths
import tempfile as temporary


class Local:
    def __init__(self) -> None:
        pass

    def mkdir(self, mode: int) -> int:
        return mode + 1

    def unlink(self, missing_ok: bool) -> bool:
        return not missing_ok

    def rmdir(self) -> str:
        return "local rmdir"

    def replace(self, target: str) -> str:
        return target + " local"


print(Local().mkdir(40))
print(Local().unlink(False))
print(Local().rmdir())
print(Local().replace("value"))
with Workspace(dir=Location("scratch"), prefix="alias-") as imported:
    paths.Path(imported, "child").mkdir()
    print(paths.Path(imported, "child").is_dir())
print(Location(imported).exists())
with temporary.TemporaryDirectory(dir="scratch") as qualified:
    print(Location(qualified).is_dir())
print(Location(qualified).exists())
