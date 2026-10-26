import pkgutil


def resource(filename: str) -> bytes | None:
    return pkgutil.get_data(__package__, filename)
