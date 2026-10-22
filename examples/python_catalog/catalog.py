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
class Target:
    triple: Triple
    variant: str | None
    key: tuple[str, str, str, str]
    priority: tuple[int, int]


@dataclass(slots=True, frozen=True)
class Download:
    version: Version
    target: Target
    filename: str
    url: str
    build: str
    sha256: str | None


@dataclass(slots=True)
class Group:
    selected: dict[tuple[str, str, str, str], Download]


@dataclass(slots=True)
class Selection:
    groups: dict[tuple[int, int, int, str], Group]
    targets: dict[tuple[str, str], Target]


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
    debug = contains(options, "debug")
    freethreaded = contains(options, "freethreaded")
    if debug and freethreaded:
        return "freethreaded+debug"
    if debug:
        return "debug"
    if freethreaded:
        return "freethreaded"
    return None


def parse_target(platform: str, variant: str) -> Target | None:
    options: list[str] = []
    if platform.endswith("-debug"):
        platform = platform[:-len("-debug")]
        options.append("debug")
    if platform.endswith("-freethreaded"):
        platform = platform[:-len("-freethreaded")]
        options.append("freethreaded")
    flavor = parse_variant(variant, options)
    if contains(options, "static"):
        return None
    triple = normalize_triple(platform)
    if triple is None:
        return None
    normalized_variant = build_variant(options)
    key = (triple.platform, arch_key(triple.arch), triple.libc, str(normalized_variant or ""))
    optimized = int(contains(options, "lto")) + int(contains(options, "pgo")) + 1
    return Target(triple, normalized_variant, key, (flavor_priority(flavor), -optimized))


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


def arch_key(arch: Arch) -> str:
    variant = arch.variant
    return arch.family + "_" + variant if variant else arch.family


def artifact_key(artifact: Artifact) -> str:
    return artifact.url


def select_line(selection: Selection, raw_line: str) -> None:
    line = raw_line.strip()
    if not line:
        return
    record = decode(line, Release)
    version_text, _separator, date = record.version.partition("+")
    version = parse_version(version_text)
    release = int(date)
    build = str(release)
    version_key = (version.major, version.minor, version.patch, version.prerelease)
    group = selection.groups.get(version_key)
    for artifact in sorted(record.artifacts, key=artifact_key):
        filename = unquote(artifact.url.rsplit("/", maxsplit=1)[-1])
        labels = (artifact.platform, artifact.variant)
        target = selection.targets.get(labels)
        if target is None:
            target = parse_target(artifact.platform, artifact.variant)
            if target is None:
                continue
            if len(selection.targets) < 512:
                selection.targets[labels] = target
        if release < 20250311 and target.triple.libc == "musl":
            continue
        if group is None:
            group = Group({})
            selection.groups[version_key] = group
        previous = group.selected.get(target.key)
        if previous is not None and target.priority >= previous.target.priority:
            continue
        # On ties, the first winner keeps its value and insertion position.
        group.selected[target.key] = Download(version, target, filename, artifact.url, build, artifact.sha256)


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
    triple = download.target.triple
    return (
        -version.major, -version.minor, -version.patch, -kind, -number,
        variant_order(download.target.variant), triple.platform, triple.arch.family,
        str(triple.arch.variant or ""), triple.libc,
    )


def download_key(download: Download) -> str:
    version = download.version
    key = f"cpython-{version.major}.{version.minor}.{version.patch}{version.prerelease}"
    variant = download.target.variant
    if variant:
        key += "+" + variant
    triple = download.target.triple
    return key + "-" + triple.platform + "-" + arch_key(triple.arch) + "-" + triple.libc


def finish_catalog(selection: Selection) -> dict[str, CatalogEntry]:
    downloads: list[Download] = []
    for _version_key, group in selection.groups.items():
        for _selection_key, chosen in group.selected.items():
            downloads.append(chosen)
    results: dict[str, CatalogEntry] = {}
    for download in sorted(downloads, key=download_sort_key):
        version = download.version
        if (version.major, version.minor) < (3, 8):
            continue
        triple = download.target.triple
        results[download_key(download)] = CatalogEntry(
            "cpython", triple.arch, triple.platform, triple.libc,
            version.major, version.minor, version.patch, version.prerelease,
            download.url, download.sha256, download.target.variant, download.build,
        )
    return results


def generate_catalog(text: str) -> dict[str, CatalogEntry]:
    selection = Selection({}, {})
    for line in text.splitlines():
        select_line(selection, line)
    return finish_catalog(selection)
