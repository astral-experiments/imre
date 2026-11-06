from pathlib import PureWindowsPath


def archive_target(server: PureWindowsPath, directory: PureWindowsPath, name: str) -> str:
    return server.joinpath(directory, "packages", name).with_suffix(".zip").as_posix()


server = PureWindowsPath("//fileserver")
root = PureWindowsPath("C:/build")
filename = "report.csv"
print(archive_target(server, root.relative_to(root), filename))
print(archive_target(server, PureWindowsPath("manifest.json").parent, filename))
print(archive_target(server, PureWindowsPath(""), filename))
print(archive_target(server, PureWindowsPath("."), filename))
print(archive_target(server, (root / "team").relative_to(root), filename))
print(archive_target(server, root.parent, filename))
