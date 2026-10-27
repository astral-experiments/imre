def python_version() -> str:
    return _parse_version(sys.version, sys.platform)[1]
