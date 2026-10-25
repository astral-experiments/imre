"""Show the complete top-level work queue from the pinned CPython interpreter."""
import argparse
import json
import sys
import tomllib
from collections import Counter
from pathlib import Path


def inventory():
    if sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("run this inventory with CPython 3.14.5")
    config = tomllib.loads(Path(__file__).with_name("scope.toml").read_text())
    public = {name for name in sys.stdlib_module_names if not name.startswith("_")}
    groups = {
        "wave_one": list(config["wave_one"]),
        **config["next"],
        "proposed_fringe": list(config["proposed_fringe"]),
    }
    entries = Counter(name for names in groups.values() for name in names)
    repeated = sorted(name for name, count in entries.items() if count != 1)
    unknown = sorted(entries.keys() - public)
    if repeated or unknown:
        raise SystemExit(f"invalid module assignments: repeated={repeated}, unknown={unknown}")
    groups["queued"] = sorted(public - entries.keys())
    return {group: sorted(names) for group, names in groups.items()}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    groups = inventory()
    if args.json:
        print(json.dumps(groups, indent=2))
    else:
        for group, names in groups.items():
            print(f"{group} ({len(names)}): {', '.join(names)}")
