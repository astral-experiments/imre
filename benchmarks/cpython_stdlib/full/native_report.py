"""Report validated native evidence separately from reference and legacy results."""
import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import sys
import tomllib

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from reference_report import checked_reference
sys.path.insert(0, str(HERE.parent))
from suite_evidence import validate_pair


def verified_counts(pair, reference):
    validate_pair(pair, reference)
    observations = [{(r["id"], r["occurrence"], r["fixture_variant"]): r["classification"]
                     for r in report["records"]} for report in pair]
    if observations[0].keys() != observations[1].keys():
        raise ValueError("the O0 and O3 reports disagree about an upstream fixture variant")
    both = sum(a == observations[1][key] == "fully-native" for key, a in observations[0].items())
    return both, {f"O{r['opt_level']}": r["counts"] for r in pair}


def rollup(root, pin, platform):
    assigned = tomllib.loads((root / "scope.toml").read_text())["wave_one"]
    rows = {}
    expected = set()
    ledgers = sorted(root.glob(f"*/reference*-{platform}.json"))
    for path in ledgers:
        mod = path.parent.name
        raw, cases = checked_reference(path, pin, platform)
        row = rows.setdefault(mod, {"instances": 0, "ledger_instances": 0, "fully_native_both": None,
                                    "O0": Counter(), "O3": Counter(), "sources": {}})
        row["instances"] += len(cases)
        # New reports have an explicit filename. Some existing native-*.json
        # files are method-level summaries; never infer roles from those.
        evidence = path.with_name(path.name.replace("reference", "evidence", 1))
        expected.add(evidence)
        if not evidence.exists():
            evidence = path.with_name(path.name.replace("reference", "native", 1))
            if not evidence.exists():
                row["sources"][path.name] = "no occurrence ledger"
                continue
            pair = json.loads(evidence.read_bytes())
            if isinstance(pair, dict) and "schema" not in pair:
                row["sources"][path.name] = f"legacy: {evidence.name}"
                continue
        else:
            pair = json.loads(evidence.read_bytes())
        if not isinstance(pair, list):
            raise ValueError(f"{evidence}: expected two versioned occurrence reports")
        outcomes = defaultdict(list)
        for name, outcome in cases:
            outcomes[name].append(outcome)
        reference = {"module": raw.get("module") or "test." + assigned[mod], "platform": platform,
                     "cpython_commit": pin, "denominator": len(cases), "reference": outcomes}
        both, levels = verified_counts(pair, reference)
        row["ledger_instances"] += len(cases)
        row["fully_native_both"] = (row["fully_native_both"] or 0) + both
        for level in ("O0", "O3"):
            row[level].update(levels[level])
        row["sources"][path.name] = evidence.name
    if missing := set(assigned) - rows.keys():
        raise ValueError(f"missing {platform} reference results for {sorted(missing)}")
    if unmatched := set(root.glob(f"*/evidence*-{platform}.json")) - expected:
        raise ValueError(f"no matching reference for {sorted(str(p) for p in unmatched)}")
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", default="linux")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    pin = json.loads((HERE.parent / "UPSTREAM.json").read_bytes())["commit"]
    rows = rollup(HERE, pin, args.platform)
    covered = sum(r["ledger_instances"] for r in rows.values())
    total = {"instances": sum(r["instances"] for r in rows.values()), "ledger_instances": covered,
             "fully_native_both": sum(r["fully_native_both"] or 0 for r in rows.values()) if covered else None}
    if args.json:
        print(json.dumps({"cpython_commit": pin, "platform": args.platform, "modules": rows, "total": total}, indent=2))
    else:
        print(f"{'module':<16}{'reference':>13}{'in ledger':>13}{'native O0 & O3':>17}")
        for name, row in [*sorted(rows.items()), ("TOTAL", total)]:
            native = row["fully_native_both"]
            print(f"{name:<16}{row['instances']:>13}{row['ledger_instances']:>13}{native if native is not None else '—':>17}")


if __name__ == "__main__":
    main()
