# Import side effects must stop at the first initialization failure.
# ruff: noqa: F401
import broken
import unreached

print("must not run")
