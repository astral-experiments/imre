"""Run unittest from the command line."""

import os.path
import sys

from unittest import main

if sys.argv[0].endswith("__main__.py"):
    sys.argv[0] = os.path.basename(sys.executable) + " -m unittest"

main(module=None)
