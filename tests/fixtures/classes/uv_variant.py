"""uv's Variant declaration and variant_sort_key from its pinned catalog renderer."""

from enum import StrEnum


class Variant(StrEnum):
    FREETHREADED = "freethreaded"
    DEBUG = "debug"
    FREETHREADED_DEBUG = "freethreaded+debug"

    @classmethod
    def from_build_options(
        cls: type["Variant"], build_options: list[str]
    ) -> "Variant | None":
        if "debug" in build_options and "freethreaded" in build_options:
            return cls.FREETHREADED_DEBUG
        elif "debug" in build_options:
            return cls.DEBUG
        elif "freethreaded" in build_options:
            return cls.FREETHREADED
        else:
            return None


def variant_sort_key(variant: Variant | None) -> int:
    if variant is None:
        return 0
    match variant:
        case Variant.FREETHREADED:
            return 1
        case Variant.FREETHREADED_DEBUG:
            return 2
        case Variant.DEBUG:
            return 3
    raise ValueError(f"Missing sort key implementation for variant: {variant}")


options: list[list[str]] = [
    ["debug"], ["debug", "freethreaded"], [], ["unrecognized"],
    ["freethreaded"], ["debug", "debug"], ["freethreaded", "debug"],
]
variants: list[Variant | None] = [Variant.from_build_options(item) for item in options]
for variant in sorted(variants, key=variant_sort_key):
    print(variant_sort_key(variant))
    print("none" if variant is None else variant.value)
