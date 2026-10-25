from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class Build:
    artifact: Artifact = field(default_factory=lambda: Artifact())


@dataclass
class Artifact:
    names: list[str] = field(default_factory=list)


@dataclass
class Archive:
    artifact: Artifact | None = field(default_factory=Artifact)


def run() -> None:
    first = Build()
    first.artifact.names.append("first")
    second = Build()
    print(len(second.artifact.names))
    second.artifact.names.append("second")
    print(first.artifact.names[0])
    print(Build(first.artifact).artifact.names[0])
    archived = Archive()
    artifact = archived.artifact
    if artifact is not None:
        print(len(artifact.names))
    print(Archive(None).artifact is None)


run()
