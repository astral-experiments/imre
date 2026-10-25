"""Render the public stdlib support table with the pinned CPython and a Linux Imre build."""
import argparse
import difflib
import json
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from inventory import inventory
from native_report import rollup

ROOT = HERE.parents[2]
PAGE = ROOT / "docs" / "stdlib.md"


def import_status(compiler, source, name):
    source.write_text(f"import {name}\n")
    accepted = []
    for flags in [(), ("--reachable-only",)]:
        result = subprocess.run(
            [compiler, "check", source, "--module-root", source.parent, *flags],
            cwd=source.parent, capture_output=True, text=True, timeout=30,
        )
        if result.returncode and (result.returncode != 1 or ": Imre:" not in result.stderr):
            raise RuntimeError(f"Could not probe {name}: {result.stderr}")
        accepted.append(result.returncode == 0)
    ordinary, reachable = accepted
    if ordinary:
        return "Yes" if reachable else "Whole only"
    return "Reachable only" if reachable else "Rejected"


def measurements(name, rows):
    row = rows.get(name)
    if row is None:
        return "—", "—", "—"
    native = row["fully_native_both"]
    total, covered = row["instances"], row["ledger_instances"]
    fraction = f"{native if native is not None else '—'} / {total:,}"
    if covered != total:
        fraction += f" ({covered:,} assessed)"
    levels = row["O3"]
    return (
        f"[{fraction}](../benchmarks/cpython_stdlib/full/{name}/)",
        str(levels["native-body-host-fixture"]) if covered else "—",
        str(levels["host-driven-native-operation"]) if covered else "—",
    )


def render(compiler):
    groups = inventory()
    names = sorted(name for group in groups.values() for name in group)
    registered = sys.stdlib_module_names
    underscored = sorted(registered - set(names))
    pin = json.loads((HERE.parent / "UPSTREAM.json").read_text())["commit"]
    rows = rollup(HERE, pin, "linux")
    examples = tomllib.loads((HERE / "api_examples.toml").read_text())
    fringe = tomllib.loads((HERE / "scope.toml").read_text())["proposed_fringe"]
    if unknown := examples.keys() - registered:
        raise ValueError(f"Examples for unknown modules: {sorted(unknown)}")
    with tempfile.TemporaryDirectory() as temp:
        source = Path(temp) / "check_import.py"
        imports = {name: import_status(compiler, source, name) for name in sorted(registered)}
        main_import = import_status(compiler, source, "__main__")

    def has_detail(name):
        return name in examples or name in rows or imports[name] != "Rejected"

    supported = [name for name in names if has_detail(name)]
    other = [name for name in names if name not in supported and name not in fringe]
    excluded = [name for name in fringe if name not in supported]
    measured = [rows[name] for name in registered if name in rows]
    native = sum(row["fully_native_both"] or 0 for row in measured)
    denominator = sum(row["instances"] for row in measured)
    covered = sum(row["ledger_instances"] for row in measured)
    host_fixtures = sum(row["O3"]["native-body-host-fixture"] for row in measured)
    host_tests = sum(row["O3"]["host-driven-native-operation"] for row in measured)
    lines = [
        "# Standard library support",
        "",
        f"Imre checks every name in the [CPython 3.14.5](https://github.com/python/cpython/tree/{pin}/Lib) top-level module registry (`sys.stdlib_module_names`) on Linux. The first tables cover {len(names)} names without a leading underscore; [the appendix](#names-beginning-with-an-underscore) covers the other {len(underscored)}. Dotted APIs such as `os.path` appear under their parent.",
        "",
        f"**At a glance:** {sum(result in ('Yes', 'Whole only') for result in imports.values())} of {len(registered)} modules accept an ordinary import; {sum(result == 'Reachable only' for result in imports.values())} more accept it only with `--reachable-only`. Across {len(measured)} modules with recorded suites, {native:,} complete test executions pass natively at both optimization levels; {covered:,} of {denominator:,} upstream executions have a classification.",
        "",
        "**Import** checks an otherwise empty file containing `import module`, in ordinary and reachable-only compilation. A successful import does not establish that any function works. Follow the evidence links for the supported operations and their limits.",
        "",
        "**Native / upstream** counts unchanged test executions in the checked-in Linux suites: the body, assertions, operation and fixtures must all run on Imre at **both O0 and O3**. The denominator includes upstream skips, companion suites and repeated test IDs. A dash means there is no measurement; zero is a measured result. These are test counts, not percentages of an API.",
        "",
        f"The last two columns show where CPython still helps at O3. **CPython fixture**: {host_fixtures:,} test bodies run on Imre with CPython providing part or all of the fixture (for example, setup or test objects). **CPython test**: {host_tests:,} executions exercise Imre operations with CPython running the test body or checking its assertions. Neither count contributes to the native numerator.",
        "",
        "## Modules with examples, accepted imports or suite results",
        "",
        "| Module | Import | Native API example or boundary | Native / upstream | CPython fixture | CPython test |",
        "| --- | --- | --- | ---: | ---: | ---: |",
    ]
    def module_row(name):
        note = "No API example recorded."
        if name in examples:
            note, path = examples[name]
            if not (ROOT / path).is_file():
                raise ValueError(f"{name}: missing example {path}")
            note += f" [Evidence](../{path})"
        native, fixture, test = measurements(name, rows)
        return f"| `{name}` | {imports[name]} | {note} | {native} | {fixture} | {test} |"

    lines.extend(module_row(name) for name in supported)

    lines.extend(["", "## Modules without an accepted import or a suite measurement", "",
                  "Both import probes reject these names. No native API example or full-suite ledger is recorded here.", "",
                  "| Module |", "| --- |"])
    lines += [f"| `{name}` |" for name in other]
    lines.extend(["", "## Proposed scope exclusions", "",
                  "These names are still probed. The reasons come from the [scope inventory](../benchmarks/cpython_stdlib/full/scope.toml); they do not exempt any tests in a measured suite.", "",
                  "| Module | Import | Reason |", "| --- | --- | --- |"])
    lines += [f"| `{name}` | {imports[name]} | {fringe[name]} |" for name in sorted(excluded)]
    lines.extend(["", "## Names beginning with an underscore", "",
                  "These include documented interfaces such as `__future__` and `_thread`, as well as implementation details. The same import and suite checks apply.", "",
                  "<details>", f"<summary>Show {len(underscored)} module names</summary>", "",
                  "| Module | Import | Native API example or boundary | Native / upstream | CPython fixture | CPython test |",
                  "| --- | --- | --- | ---: | ---: | ---: |"])
    lines.extend(module_row(name) for name in underscored if has_detail(name))
    if rejected := [name for name in underscored if not has_detail(name)]:
        lines.extend(["", "Both import modes reject the following names. They have no recorded API example or suite measurement.", "",
                      "| Module |", "| --- |"])
        lines.extend(f"| `{name}` |" for name in rejected)
    lines.extend(["", "</details>", "",
                  f"The entry environment, `__main__`, is separate from CPython’s module registry. Imre’s `import __main__` probe: **{main_import}**. The [entry-point guard](../tests/fixtures/module_values/entry_globals/main.py) uses `if __name__ == '__main__'`."])
    if extra := sorted(rows.keys() - registered):
        lines.extend(["", "## Additional test subjects", "",
                      "These ledgers describe Python types or other subjects outside the top-level module inventory.", "",
                      "| Subject | Native / upstream | CPython fixture | CPython test |", "| --- | ---: | ---: | ---: |"])
        for name in extra:
            native, fixture, test = measurements(name, rows)
            lines.append(f"| `{name}` | {native} | {fixture} | {test} |")
    lines.extend(["", "## Updating this page", "",
                  "Add an [API example](../benchmarks/cpython_stdlib/full/api_examples.toml) when a module gains a usable operation. Update the [per-test evidence](../benchmarks/cpython_stdlib/full/README.md) when native execution changes. From the repository root, use a current Linux Imre build and CPython 3.14.5:", "",
                  "```sh", "python3.14 -B benchmarks/cpython_stdlib/full/support_table.py --imre target/release/imre", "```", "",
                  "Pass `--check` to verify the checked-in page."])
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if not sys.platform.startswith("linux"):
        parser.error("generate the Linux support table on Linux")
    result = render(args.imre.resolve())
    if args.check:
        old = PAGE.read_text()
        if old != result:
            sys.stderr.writelines(difflib.unified_diff(old.splitlines(True), result.splitlines(True),
                                                      fromfile=str(PAGE), tofile="regenerated"))
            parser.exit(1, "stdlib support table is stale; rerun without --check\n")
    else:
        PAGE.write_text(result)


if __name__ == "__main__":
    main()
