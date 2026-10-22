"""Run the unchanged upstream CPython finder and renderer without networking."""

import asyncio
import importlib.util
import logging
import sys
import types
from pathlib import Path

UPSTREAM = Path(__file__).parent / "upstream/fetch-download-metadata.py"


class OfflineResponse:
    def __init__(self, text: str):
        self.text = text

    def raise_for_status(self) -> None:
        pass


class OfflineClient:
    def __init__(self, text: str, expected_url: str):
        self.text = text
        self.expected_url = expected_url
        self.requests = 0

    async def get(self, url: str) -> OfflineResponse:
        if url != self.expected_url or self.requests:
            raise AssertionError("the offline adapter permits exactly one CPython index request")
        self.requests += 1
        return OfflineResponse(self.text)


class MemoryOutput:
    """Keep the unchanged renderer's complete text without repeated file I/O."""

    def __init__(self):
        self.parent = self
        self.text = ""

    def mkdir(self, *, parents: bool, exist_ok: bool) -> None:
        assert parents and exist_ok

    def write_text(self, text: str, *, newline: str) -> int:
        assert newline == "\n"
        self.text = text
        return len(text)


def load_upstream():
    # httpx is used only for annotations and the replaced network boundary in
    # this mode. Supplying that interface avoids an unrelated package install.
    httpx = types.ModuleType("httpx")
    httpx.AsyncClient = OfflineClient
    previous = sys.modules.get("httpx")
    sys.modules["httpx"] = httpx
    try:
        name = "_relar_upstream_cpython_catalog"
        spec = importlib.util.spec_from_file_location(name, UPSTREAM)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        spec.loader.exec_module(module)
    finally:
        if previous is None:
            del sys.modules["httpx"]
        else:
            sys.modules["httpx"] = previous
    module.logger.setLevel(logging.CRITICAL)
    return module


def run(text: str, output: Path | MemoryOutput, module=None) -> int:
    if module is None:
        module = load_upstream()
    client = OfflineClient(text, module.CPythonFinder.NDJSON_URL)
    downloads = asyncio.run(module.CPythonFinder(client).find())
    assert client.requests == 1
    module.VERSIONS_FILE = output
    module.render(downloads)
    return len(downloads)


def compute(rounds: int, source: Path, output: Path) -> int:
    if rounds < 0:
        raise ValueError("rounds must be nonnegative")
    text = source.read_text(encoding="utf-8")
    module = load_upstream()
    sink = MemoryOutput()
    checksum = 0
    for _ in range(rounds):
        run(text, sink, module)
        checksum += len(sink.text)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(sink.text, encoding="utf-8", newline="\n")
    return checksum


def main() -> None:
    if len(sys.argv) == 5 and sys.argv[1] == "--compute":
        print(compute(int(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4])))
        return
    if len(sys.argv) != 3:
        raise ValueError("expected INPUT.ndjson OUTPUT.json")
    text = Path(sys.argv[1]).read_text(encoding="utf-8")
    run(text, Path(sys.argv[2]))


if __name__ == "__main__":
    main()
