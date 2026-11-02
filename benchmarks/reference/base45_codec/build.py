"""Build the independent Base45 reference with explicit, reproducible provenance."""

import runpy
from pathlib import Path


if __name__ == "__main__":
    root = Path(__file__).resolve().parent
    helper = root / "wrapped_reference.py"
    if not helper.is_file():
        helper = root.parent / helper.name
    build = runpy.run_path(str(helper))["build_reference"]
    build(
        root,
        __doc__,
        entrypoint="base45",
        cargo_binary="base45-reference",
        output_name="base45-reference",
        role="reference",
        comparison_domain=(
            "Typed file driver with a fixed Base45 alphabet; performance inputs are valid "
            "ASCII Base45 or arbitrary encoded bytes."
        ),
        diagnostic_adapter=(
            "Exact error text is validated over the declared corpus, not universal CPython "
            "Unicode repr equivalence. Rust Unicode quoting can differ for combining marks "
            "or Unicode-version boundaries; those cases validate category/cause/suppression "
            "and unchanged output separately."
        ),
    )
