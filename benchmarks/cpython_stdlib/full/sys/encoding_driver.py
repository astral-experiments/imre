"""Transport native results as Python literals, including their types."""
import sys
from sys import getdefaultencoding

def main() -> None:
    command = sys.argv[1]
    if command == "default":
        print(repr(getdefaultencoding()))
    elif command == "default-argument":
        # The upstream method deliberately invokes this signature incorrectly.
        print(repr(getdefaultencoding(int(sys.argv[2]))))  # ty: ignore[too-many-positional-arguments]
    else:
        raise ValueError("unknown encoding operation")

main()
