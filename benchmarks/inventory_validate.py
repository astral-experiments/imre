"""Independent cumulative-demand and filesystem oracle for inventory reconciliation."""

import argparse
import json
import posixpath
import random
import re
import runpy
import subprocess
import tempfile
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_ROOT = ROOT / "examples"
PACKAGE = PACKAGE_ROOT / "inventory_reconciliation"
ENTRY = PACKAGE / "__main__.py"
files = SimpleNamespace(**runpy.run_path(str(ROOT / "benchmarks/manifest_validate.py")))
Case = files.Case
digest = files.digest
case_record = files.case_record
case_from_record = files.case_from_record
materialize = files.materialize
snapshot = files.snapshot
encoded_files = files.encoded_files
BOOTSTRAP = (
    "import runpy, sys; sys.path.insert(0, sys.argv[1]); "
    "sys.argv = ['inventory_reconciliation', *sys.argv[2:]]; "
    "runpy.run_module('inventory_reconciliation', run_name='__main__', alter_sys=True)"
)
ARGUMENTS = ("stock.txt", "orders.txt", "report.txt")


def package_sources() -> list[Path]:
    return sorted(PACKAGE.rglob("*.py"))


def python_command(python: str) -> list[str]:
    return [python, "-I", "-c", BOOTSTRAP, str(PACKAGE_ROOT)]


def inputs(
    stocks: list[tuple[str, int]], batches: list[list[tuple[str, int]]]
) -> dict[str, bytes]:
    stock = [str(len(stocks)), *(f"{sku} {quantity}" for sku, quantity in stocks)]
    orders = [str(len(batches))]
    for batch in batches:
        orders.extend(
            (str(len(batch)), *(f"{sku} {quantity}" for sku, quantity in batch))
        )
    return {
        "stock.txt": ("\n".join(stock) + "\n").encode(),
        "orders.txt": ("\n".join(orders) + "\n").encode(),
    }


def case(
    label: str, stocks: list[tuple[str, int]], batches: list[list[tuple[str, int]]]
) -> Case:
    return Case(label, inputs(stocks, batches), ARGUMENTS)


def parse(text: str, label: str):
    """Independent bounded parsing; ASCII decimal is the shared input domain."""
    rows = iter(text.splitlines())

    def fields() -> list[str]:
        try:
            return next(rows).split()
        except StopIteration:
            raise ValueError(label + ": truncated") from None

    def number(token: str) -> int:
        if re.fullmatch(r"[+-]?[0-9]+(?:_[0-9]+)*", token) is None:
            raise ValueError(label + ": invalid integer")
        return int(token)

    def count() -> int:
        value = fields()
        if len(value) != 1:
            raise ValueError(label + ": expected count")
        result = number(value[0])
        if result < 0:
            raise ValueError(label + ": negative count")
        return result

    def item() -> tuple[str, int]:
        value = fields()
        if len(value) != 2:
            raise ValueError(label + ": expected SKU and quantity")
        return value[0], number(value[1])

    if label == "stock":
        result = []
        for _ in range(count()):
            sku, quantity = item()
            if quantity < 0:
                raise ValueError("stock: negative quantity")
            result.append((sku, quantity))
    else:
        result = [[item() for _ in range(count())] for _ in range(count())]
    if next(rows, None) is not None:
        raise ValueError(label + ": trailing data")
    return result


def report(stocks: list[tuple[str, int]], batches: list[list[tuple[str, int]]]) -> str:
    """Compute each allocation from initial stock minus prior positive demand.

    This never mutates a remaining-stock map while processing requests and does
    not import the compiled package or its fulfillment/rendering helpers.
    """
    initial = dict(stocks)
    demand = {}
    results = []
    for batch_id, batch in enumerate(batches):
        for row_id, (sku, requested) in enumerate(batch):
            if requested <= 0:
                continue
            previous = demand.get(sku, 0)
            filled = min(requested, max(initial.get(sku, 0) - previous, 0))
            demand[sku] = previous + requested
            results.append(
                (batch_id, row_id, sku, requested, filled, requested - filled)
            )
    remaining = {
        sku: max(quantity - demand.get(sku, 0), 0) for sku, quantity in initial.items()
    }
    shortages = [item for item in results if item[-1] > 0]
    untouched = [
        (sku, quantity) for sku, quantity in remaining.items() if sku not in demand
    ]
    lines = [
        "INVENTORY",
        f"STOCK_ROWS {len(stocks)}",
        f"STOCK_SKUS {len(initial)}",
        f"BATCHES {len(batches)}",
        f"REQUEST_ROWS {sum(map(len, batches))}",
        f"POSITIVE_REQUESTS {len(results)}",
        f"REQUESTED_SKUS {len(demand)}",
        f"TOTAL_REQUESTED {sum(item[3] for item in results)}",
        f"TOTAL_FILLED {sum(item[4] for item in results)}",
        f"TOTAL_SHORTAGE {sum(item[5] for item in results)}",
        f"FULFILLMENTS {len(results)}",
    ]
    lines.extend("FULFILL " + " ".join(map(str, item)) for item in results)
    lines.append(f"SHORTAGES {len(shortages)}")
    lines.extend(
        f"SHORTAGE {batch} {row} {sku} {shortage}"
        for batch, row, sku, _, _, shortage in shortages
    )
    lines.append(f"REQUESTED {len(demand)}")
    lines.extend(f"REQUEST {sku} {quantity}" for sku, quantity in demand.items())
    lines.append(f"REMAINING {len(remaining)}")
    lines.extend(f"STOCK {sku} {quantity}" for sku, quantity in remaining.items())
    lines.append(f"UNTOUCHED {len(untouched)}")
    lines.extend(f"STOCK {sku} {quantity}" for sku, quantity in untouched)
    lines.append("END")
    assert all(quantity >= 0 for quantity in remaining.values())
    assert sum(item[3] for item in results) == sum(
        item[4] + item[5] for item in results
    )
    allocated = {}
    for item in results:
        allocated[item[2]] = allocated.get(item[2], 0) + item[4]
    for sku, quantity in initial.items():
        assert allocated.get(sku, 0) + remaining[sku] == quantity
    return "\n".join(lines) + "\n"


def oracle(value: Case) -> dict:
    nodes = {name: [data] for name, data in value.files.items()}
    for name, target in value.hardlinks.items():
        nodes[name] = nodes[target]
    directories = files.directory_names(value)
    category = message = None

    def resolve(path: str) -> str:
        if "\0" in path:
            raise ValueError
        if not path:
            raise OSError
        prefix = []
        for component in path.split("/")[:-1]:
            if component == "..":
                if not prefix:
                    raise OSError
                prefix.pop()
            elif component not in ("", "."):
                prefix.append(component)
            if ("/".join(prefix) or ".") not in directories:
                raise OSError
        name = posixpath.normpath(path)
        parent = posixpath.dirname(name)
        if name in directories or (parent and parent not in directories):
            raise OSError
        return name

    def load(path: str, label: str) -> str:
        try:
            name = resolve(path)
            if name not in nodes:
                raise OSError
            return nodes[name][0].decode("utf-8")
        except UnicodeDecodeError:
            raise ValueError(label + ": invalid UTF-8") from None
        except OSError:
            raise OSError(label + ": cannot read") from None
        except ValueError:
            raise ValueError(label + ": invalid path") from None

    try:
        if len(value.arguments) != 3:
            raise ValueError("usage: inventory_reconciliation STOCK ORDERS REPORT")
        stocks = parse(load(value.arguments[0], "stock"), "stock")
        batches = parse(load(value.arguments[1], "orders"), "orders")
        output = report(stocks, batches).encode()
        try:
            destination = resolve(value.arguments[2])
            if destination not in nodes:
                nodes[destination] = [b""]
            # Truncation/writes update every hardlink; this is not atomic output.
            nodes[destination][0] = output
        except OSError:
            raise OSError("report: cannot write") from None
        except ValueError:
            raise ValueError("report: invalid path") from None
    except (OSError, ValueError) as error:
        category, message = type(error).__name__, str(error)
    return {
        "returncode": int(category is not None),
        "error_category": category,
        "error_message": message,
        "usage_message": message,
        "stdout": "",
        "files": {name: node[0] for name, node in nodes.items()},
        "directories": sorted(directories - {"."}),
    }


def check_result(value: Case, expected: dict, process, directory: Path) -> dict:
    result = files.check_result(value, expected, process, directory)
    if expected["error_category"] is not None:
        last = process.stderr.decode().strip().splitlines()[-1]
        assert last == expected["error_category"] + ": " + expected["error_message"], (
            value.label,
            last,
        )
    return result


def cases() -> list[Case]:
    huge = 10**100
    result = [
        case("empty", [], []),
        case("empty-batches", [("untouched", 8)], [[], [], []]),
        case(
            "duplicates-shortages",
            [("a", 8), ("b", 4), ("a", 3)],
            [[("a", 2), ("a", 4), ("missing", 1)], [("b", 0), ("b", -7), ("b", 3)]],
        ),
        case("ignored-unknown", [("keep", 5)], [[("unknown", 0), ("keep", -1)]]),
        case(
            "unicode-and-nul-skus",
            [("café", 7), ("大きい", huge), ("a\0b", 2)],
            [[("大きい", huge + 1), ("café", 3), ("a\0b", 1)]],
        ),
        case(
            "huge-cumulative-demand",
            [("large", huge + 5)],
            [[("large", huge)], [("large", huge), ("large", huge)]],
        ),
        case(
            "tag-and-i64-boundaries",
            [(str(i), i) for i in (0, 2**62 - 1, 2**62, 2**63 - 1, 2**63, 2**64)],
            [[(str(i), i + 1) for i in (0, 2**62 - 1, 2**62, 2**63 - 1, 2**63, 2**64)]],
        ),
    ]
    rng = random.Random(20261018)
    names = ["a", "b", "café", "部品", "zero", "missing"]
    for index in range(100):
        stocks = [
            (rng.choice(names[:-1]), rng.randrange(90))
            for _ in range(rng.randrange(18))
        ]
        batches = [
            [
                (
                    rng.choice(names),
                    rng.randrange(-4, 25) + (huge if index % 17 == 0 else 0),
                )
                for _ in range(rng.randrange(15))
            ]
            for _ in range(rng.randrange(8))
        ]
        result.append(case(f"random-{index}", stocks, batches))
    valid = inputs([("a", 5)], [[("a", 2)]])
    for token in ("+0", "-0", "-0_0", "+0_0", "0" * 18, "0" * 19):
        for kind in ("stock", "orders"):
            result.append(
                Case(
                    f"{kind}-zero-count-{token}",
                    {**inputs([], []), f"{kind}.txt": (token + "\n").encode()},
                    ARGUMENTS,
                )
            )
    invalid = [
        ("empty", b""),
        ("negative-count", b"-1\n"),
        ("bad-count", b"x\n"),
        ("count-fields", b"1 2\n"),
        ("blank-count", b"\n"),
        ("truncated", b"1\n"),
        ("huge-count", (str(huge) + "\n").encode()),
        ("bad-underscore", b"1__0\n"),
        ("leading-underscore", b"_1\n"),
        ("trailing-underscore", b"-0_\n"),
        ("sign-plus-only", b"+\n"),
        ("sign-minus-only", b"-\n"),
        ("double-sign", b"+-1\n"),
        ("sign-underscore", b"+_1\n"),
        ("short-count-boundary", b"999999999999999999\n"),
        ("long-count-boundary", b"9999999999999999999\n"),
        ("bom", b"\xef\xbb\xbf0\n"),
        ("trailing", b"0\nextra\n"),
        ("trailing-blank", b"0\n\n"),
        ("utf8", b"\xff\xfe"),
    ]
    for label, data in invalid:
        for kind in ("stock", "orders"):
            result.append(
                Case(
                    f"{kind}-{label}",
                    {**valid, f"{kind}.txt": data, "report.txt": b"untouched"},
                    ARGUMENTS,
                )
            )
    for label, stock in (
        ("negative-stock", b"1\na -1\n"),
        ("negative-overwritten-stock", b"2\na -1\na 4\n"),
        ("stock-row-fields", b"1\na 1 extra\n"),
        ("stock-bad-quantity", b"1\na 0x10\n"),
    ):
        result.append(Case(label, {**valid, "stock.txt": stock}, ARGUMENTS))
    for label, orders in (
        ("negative-batch-count", b"1\n-1\n"),
        ("bad-batch-count", b"1\nx\n"),
        ("orders-row-fields", b"1\n1\na\n"),
        ("orders-bad-quantity", b"1\n1\na _1\n"),
    ):
        result.append(Case(label, {**valid, "orders.txt": orders}, ARGUMENTS))
    for label, name, text in (
        ("huge-stock-count-invalid-row", "stock.txt", f"{huge}\na x\n"),
        ("huge-stock-count-negative-row", "stock.txt", f"{huge}\na -1\n"),
        ("huge-batch-count-negative-inner", "orders.txt", f"{huge}\n-1\n"),
        ("huge-batch-count-later-invalid-inner", "orders.txt", f"{huge}\n0\nx\n"),
        ("huge-request-count-invalid-row", "orders.txt", f"1\n{huge}\na x\n"),
    ):
        result.append(Case(label, {**valid, name: text.encode()}, ARGUMENTS))
    for separator in (
        "\n",
        "\r\n",
        "\r",
        "\v",
        "\f",
        "\x1c",
        "\x1d",
        "\x1e",
        "\x85",
        "\u2028",
        "\u2029",
    ):
        result.append(
            Case(
                "lines-" + separator.encode().hex(),
                {
                    name: data.replace(b"\n", separator.encode())
                    for name, data in valid.items()
                },
                ARGUMENTS,
            )
        )
    result.extend(
        [
            Case(
                "no-final-newline",
                {name: data.rstrip(b"\n") for name, data in valid.items()},
                ARGUMENTS,
            ),
            Case(
                "signs-underscores-whitespace",
                {
                    "stock.txt": "\u2003+0_1\u2003\na\x1f1_0\n".encode(),
                    "orders.txt": b"+1\n1\na +0_3\n",
                },
                ARGUMENTS,
            ),
            Case(
                "preexisting-report", {**valid, "report.txt": b"replace me"}, ARGUMENTS
            ),
            Case("report-is-stock", valid, ("stock.txt", "orders.txt", "stock.txt")),
            Case("report-is-orders", valid, ("stock.txt", "orders.txt", "orders.txt")),
            Case(
                "report-hardlink-stock",
                valid,
                ("stock.txt", "orders.txt", "alias.txt"),
                hardlinks={"alias.txt": "stock.txt"},
            ),
            Case(
                "same-input-and-report",
                {"both.txt": b"0\n"},
                ("both.txt", "both.txt", "both.txt"),
            ),
            Case(
                "missing-stock",
                {"orders.txt": valid["orders.txt"], "report.txt": b"keep"},
                ARGUMENTS,
            ),
            Case(
                "missing-orders",
                {"stock.txt": valid["stock.txt"], "report.txt": b"keep"},
                ARGUMENTS,
            ),
            Case(
                "stock-directory",
                {"orders.txt": valid["orders.txt"]},
                ARGUMENTS,
                ("stock.txt",),
            ),
            Case(
                "orders-directory",
                {"stock.txt": valid["stock.txt"]},
                ARGUMENTS,
                ("orders.txt",),
            ),
            Case("report-directory", valid, ARGUMENTS, ("report.txt",)),
            Case(
                "missing-report-parent",
                valid,
                ("stock.txt", "orders.txt", "absent/report.txt"),
            ),
            Case(
                "file-report-parent",
                {**valid, "parent": b"file"},
                ("stock.txt", "orders.txt", "parent/report.txt"),
            ),
            Case("empty-report-path", valid, ("stock.txt", "orders.txt", "")),
            Case("stock-parse-precedes-orders-open", {"stock.txt": b"x\n"}, ARGUMENTS),
            Case(
                "orders-parse-precedes-report-open",
                {**valid, "orders.txt": b"x\n"},
                ARGUMENTS,
                ("report.txt",),
            ),
            Case(
                "relative-normalization",
                valid,
                ("folder/../stock.txt", "orders.txt", "folder/report.txt"),
                ("folder",),
            ),
            Case(
                "spaces-unicode-paths",
                {"stock café.txt": valid["stock.txt"], "注文.txt": valid["orders.txt"]},
                ("stock café.txt", "注文.txt", "report café.txt"),
            ),
            Case("too-few-arguments", valid, ("stock.txt", "orders.txt")),
            Case("too-many-arguments", valid, (*ARGUMENTS, "extra")),
        ]
    )
    assert len({value.label for value in result}) == len(result)
    return result


def device_check(commands: dict[str, list[str]], timeout: float) -> dict:
    if not Path("/dev/full").exists():
        return {"skipped": "/dev/full unavailable"}
    value = case("terminal-report-device-error", [("a", 4)], [[("a", 2)]])
    results = {}
    for engine, command in commands.items():
        with tempfile.TemporaryDirectory(prefix="inventory-device-") as temporary:
            directory = Path(temporary)
            materialize(value, directory)
            process = subprocess.run(
                [*command, "stock.txt", "orders.txt", "/dev/full"],
                input=b"",
                capture_output=True,
                cwd=directory,
                timeout=timeout,
                check=False,
            )
            expected = {
                "returncode": 1,
                "error_category": "OSError",
                "error_message": "report: cannot write",
                "usage_message": "report: cannot write",
                "stdout": "",
                "files": value.files,
                "directories": [],
            }
            results[engine] = check_result(value, expected, process, directory)
    return {"case": value.label, "engines": results}


def validate_commands(commands: dict[str, list[str]], timeout: float = 30) -> dict:
    records = []
    successes = 0
    for value in cases():
        expected = oracle(value)
        successes += expected["returncode"] == 0
        results = {}
        for engine, command in commands.items():
            with tempfile.TemporaryDirectory(prefix="inventory-validate-") as temporary:
                directory = Path(temporary)
                materialize(value, directory)
                process = subprocess.run(
                    [*command, *value.arguments],
                    input=b"",
                    capture_output=True,
                    cwd=directory,
                    timeout=timeout,
                    check=False,
                )
                results[engine] = check_result(value, expected, process, directory)
        records.append(
            {
                "case": case_record(value),
                "expected": {
                    key: item for key, item in expected.items() if key != "files"
                },
                "expected_files": encoded_files(expected["files"]),
                "engines": results,
            }
        )
    return {
        "cases": len(records),
        "successful": successes,
        "terminal_errors": len(records) - successes,
        "engines": list(commands),
        "device_check": device_check(commands, timeout),
        "records": records,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python", default="python3.14")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--rust-bigint", type=Path)
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    commands = {"cpython": python_command(args.python)}
    for engine, binary in (("relar", args.binary), ("rust_bigint", args.rust_bigint)):
        if binary is not None:
            commands[engine] = [str(binary.resolve(strict=True))]
    result = validate_commands(commands, args.timeout)
    if args.output:
        args.output.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    print(
        json.dumps(
            {
                key: value
                for key, value in result.items()
                if key not in ("records", "device_check")
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
