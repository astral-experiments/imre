from pathlib import Path
import pkgutil


def describe() -> list[str]:
    directory = str(Path(__file__).parent / 'plugins')
    result: list[str] = []
    for info in pkgutil.iter_modules([directory, directory], 'parcel.plugins.'):
        finder, name, ispkg = info
        assert finder.path == directory
        assert info.name == name and info.ispkg == ispkg
        result.append(name + (' package' if ispkg else ' module'))
    return result
