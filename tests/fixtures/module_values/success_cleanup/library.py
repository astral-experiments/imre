# Keep both module aliases alive until process cleanup.
# ruff: noqa: SIM115
FILE = open("module.txt", "w", encoding="utf-8")
FILE.write("module buffer")
ALIAS = FILE


def write() -> None:
    ALIAS.write(" and call")
