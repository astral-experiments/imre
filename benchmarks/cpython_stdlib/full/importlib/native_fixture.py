from importlib.machinery import ModuleSpec as ImportedSpec


class Loader:
    pass


class Machinery:
    def ModuleSpec(
        self,
        name: str,
        loader: Loader,
        *,
        origin: str | None = None,
        is_package: bool | None = None,
    ):
        return ImportedSpec(name, loader, origin=origin, is_package=is_package)


class NativeCase:
    name: str
    path: str
    loader: Loader
    machinery: Machinery

    def __init__(self, name: str, path: str):
        self.name = name
        self.path = path
        self.loader = Loader()
        self.machinery = Machinery()

    def assertEqual(self, first: str, second: str) -> None:
        assert first == second

    def assertTrue(self, result: bool) -> None:
        assert result

    def assertFalse(self, result: bool) -> None:
        assert not result

    def assertIs(self, value: str | None, expected: None) -> None:
        assert expected is None
        assert value is None
