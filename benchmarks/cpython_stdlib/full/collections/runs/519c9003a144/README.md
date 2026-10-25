# Independent canonical release comparison

The [receipt](verified-defaults.json) records seven successful CPython
controls and the same seven original methods at O0 and O3 in both ordinary
and reachable-only compilation on canonical `519c9003a144decc4eafc9720e9934b26b170a95`.
It includes the release binary digests, source tree, compiler profile, and the
original receipt and verifier hashes. Only local artifact paths were replaced
with filenames. All test observations are unchanged.

This confirms seven records within the parent 659. [The portable verifier](../../verify_defaults.py)
can compare another compiler directly with these method and source identities.
