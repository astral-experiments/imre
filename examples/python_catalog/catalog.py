"""The CPython mode of uv's download metadata generator, with typed records."""

from dataclasses import dataclass
from urllib.parse import unquote

from relar.json import decode


@dataclass(slots=True)
class Artifact:
    url: str
    platform: str
    variant: str
    sha256: str | None = None


@dataclass(slots=True)
class Release:
    version: str
    artifacts: list[Artifact]


@dataclass(slots=True, frozen=True)
class Version:
    major: int
    minor: int
    patch: int
    prerelease: str


@dataclass(slots=True, frozen=True)
class Arch:
    family: str
    variant: str | None


@dataclass(slots=True, frozen=True)
class Triple:
    platform: str
    arch: Arch
    libc: str


@dataclass(slots=True, frozen=True)
class Download:
    release: int
    version: Version
    triple: Triple
    flavor: str
    filename: str
    url: str
    build: str
    sha256: str | None
    build_options: list[str]
    variant: str | None


@dataclass(slots=True, frozen=True)
class Choice:
    download: Download
    priority: tuple[int, int]


@dataclass(slots=True)
class Group:
    selected: dict[tuple[str, str, str, str], Choice]


@dataclass(slots=True, frozen=True)
class CatalogEntry:
    name: str
    arch: Arch
    os: str
    libc: str
    major: int
    minor: int
    patch: int
    prerelease: str
    url: str
    sha256: str | None
    variant: str | None
    build: str


def parse_version(text: str) -> Version:
    components = text.split(".", 3)
    if len(components) < 3:
        raise ValueError(f"not enough values to unpack (expected 3, got {len(components)})")
    if len(components) > 3:
        raise ValueError("too many values to unpack (expected 3)")
    major, minor, patch = components
    prerelease = ""
    for kind in ["a", "b", "rc"]:
        parts = patch.split(kind, 1)
        if len(parts) == 2:
            patch = parts[0]
            prerelease = kind + parts[1]
            break
    return Version(int(major), int(minor), int(patch), prerelease)


def normalize_arch(text: str) -> Arch:
    if text == "ppc64":
        text = "powerpc64"
    elif text == "ppc64le":
        text = "powerpc64le"
    first = text.find("_")
    if first == -1 or text.find("_", first + 1) == -1:
        return Arch(text, None)
    pieces = text.split("_")
    family = "_".join(pieces[:2])
    variant = pieces[2] if len(pieces) > 2 else None
    # Upstream's Arch equality distinguishes None from "", while its ordering
    # does not. Exclude that ambiguous architecture spelling from this port.
    if variant is not None and variant == "":
        raise ValueError("architecture variants must not be empty")
    return Arch(family, variant)


def normalize_triple(text: str) -> Triple | None:
    pieces = text.split("-")
    if len(pieces) < 3:
        return None
    if pieces[2] == "linux" and len(pieces) < 4:
        return None
    arch = normalize_arch(pieces[0])
    libc = pieces[3] if pieces[2] == "linux" else "none"
    return Triple(pieces[2], arch, libc)


def parse_variant(text: str, options: list[str]) -> str:
    if text.find("+") == -1:
        return text
    parts = text.split("+")
    last = parts[-1]
    if last == "full" or last == "install_only" or last == "install_only_stripped":
        for index in range(len(parts) - 1):
            options.append(parts[index])
        return last
    return text


def contains(values: list[str], needle: str) -> bool:
    return any(value == needle for value in values)


def build_variant(options: list[str]) -> str | None:
    if contains(options, "debug") and contains(options, "freethreaded"):
        return "freethreaded+debug"
    if contains(options, "debug"):
        return "debug"
    if contains(options, "freethreaded"):
        return "freethreaded"
    return None


def parse_artifact(version: Version, release: int, artifact: Artifact) -> Download | None:
    filename = unquote(artifact.url.rsplit("/", maxsplit=1)[-1])
    platform = artifact.platform
    options: list[str] = []
    for suffix in ["-debug", "-freethreaded"]:
        if platform.endswith(suffix):
            platform = platform[:-len(suffix)]
            options.append(suffix.lstrip("-"))
    flavor = parse_variant(artifact.variant, options)
    if contains(options, "static"):
        return None
    triple = normalize_triple(platform)
    if triple is None:
        return None
    return Download(
        release, version, triple, flavor, filename, artifact.url, str(release),
        artifact.sha256, options, build_variant(options),
    )


def flavor_priority(flavor: str) -> int:
    if flavor == "install_only_stripped":
        return 0
    if flavor == "install_only":
        return 1
    if flavor == "shared-pgo":
        return 2
    if flavor == "shared-noopt":
        return 3
    if flavor == "static-noopt":
        return 4
    return 6


def download_priority(download: Download) -> tuple[int, int]:
    options = download.build_options
    optimized = int(contains(options, "lto")) + int(contains(options, "pgo")) + int(not contains(options, "static"))
    return flavor_priority(download.flavor), -optimized


def arch_key(arch: Arch) -> str:
    variant = arch.variant
    return arch.family + "_" + variant if variant else arch.family


def artifact_key(artifact: Artifact) -> str:
    return artifact.url


def find_downloads(text: str) -> list[Download]:
    groups: dict[tuple[int, int, int, str], Group] = {}
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        record = decode(line, Release)
        version_text, _separator, date = record.version.partition("+")
        version = parse_version(version_text)
        release = int(date)
        version_key = (version.major, version.minor, version.patch, version.prerelease)
        group = groups.get(version_key)
        for artifact in sorted(record.artifacts, key=artifact_key):
            download = parse_artifact(version, release, artifact)
            if download is None:
                continue
            if download.release < 20250311 and download.triple.libc == "musl":
                continue
            if group is None:
                group = Group({})
                groups[version_key] = group
            triple = download.triple
            key = (triple.platform, arch_key(triple.arch), triple.libc, str(download.variant or ""))
            priority = download_priority(download)
            previous = group.selected.get(key)
            if previous is not None and priority >= previous.priority:
                continue
            # Updating a winner keeps the original insertion position. Selection
            # can happen while parsing because priorities have no side effects.
            group.selected[key] = Choice(download, priority)
    downloads: list[Download] = []
    for _version_key, group in groups.items():
        for _selection_key, choice in group.selected.items():
            downloads.append(choice.download)
    return downloads


def prerelease_key(text: str) -> tuple[int, int]:
    if text.startswith("a"):
        return 0, int(text[1:])
    if text.startswith("b"):
        return 1, int(text[1:])
    if text.startswith("rc"):
        return 2, int(text[2:])
    return 3, 0


def variant_order(variant: str | None) -> int:
    if variant is None:
        return 0
    if variant == "freethreaded":
        return 1
    if variant == "freethreaded+debug":
        return 2
    if variant == "debug":
        return 3
    raise ValueError("unknown build variant")


def download_sort_key(download: Download) -> tuple[int, int, int, int, int, int, str, str, str, str]:
    version = download.version
    kind, number = prerelease_key(version.prerelease)
    triple = download.triple
    return (
        -version.major, -version.minor, -version.patch, -kind, -number,
        variant_order(download.variant), triple.platform, triple.arch.family,
        str(triple.arch.variant or ""), triple.libc,
    )


def download_key(download: Download) -> str:
    version = download.version
    key = f"cpython-{version.major}.{version.minor}.{version.patch}{version.prerelease}"
    variant = download.variant
    if variant:
        key += "+" + variant
    triple = download.triple
    return key + "-" + triple.platform + "-" + arch_key(triple.arch) + "-" + triple.libc


def generate_catalog(text: str) -> dict[str, CatalogEntry]:
    results: dict[str, CatalogEntry] = {}
    for download in sorted(find_downloads(text), key=download_sort_key):
        version = download.version
        if (version.major, version.minor) < (3, 8):
            continue
        triple = download.triple
        results[download_key(download)] = CatalogEntry(
            "cpython", triple.arch, triple.platform, triple.libc,
            version.major, version.minor, version.patch, version.prerelease,
            download.url, download.sha256, download.variant, download.build,
        )
    return results
