"""Public names of shutil on targets which provide disk_usage."""

__all__: list[str] = [
    "copyfileobj", "copyfile", "copymode", "copystat", "copy", "copy2",
    "copytree", "move", "rmtree", "Error", "SpecialFileError", "make_archive",
    "get_archive_formats", "register_archive_format", "unregister_archive_format",
    "get_unpack_formats", "register_unpack_format", "unregister_unpack_format",
    "unpack_archive", "ignore_patterns", "chown", "which", "get_terminal_size",
    "SameFileError", "disk_usage",
]
