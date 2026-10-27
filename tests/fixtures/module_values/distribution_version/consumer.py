def server_version(distribution: str) -> str:
    import importlib.metadata

    return f"Server/{importlib.metadata.version(distribution)}"
