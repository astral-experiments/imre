"""Build ordinary literal-import entries for fixtures that execute at module scope."""

import shutil


def imported_fixture(directory, source):
    module = directory / source.name
    if source.resolve() != module.resolve():
        shutil.copyfile(source, module)
    entry = directory / f"import_{source.stem}.py"
    entry.write_text(f"import importlib\nimportlib.import_module({source.stem!r})\n")
    return entry
