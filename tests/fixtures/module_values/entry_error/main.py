# Initialization, rather than a member access, is the behavior under test.
# ruff: noqa: F401
import library

raise ValueError("entry failed")
