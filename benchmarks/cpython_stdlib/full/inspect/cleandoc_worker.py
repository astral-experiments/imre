import sys
from inspect import cleandoc
from pathlib import Path

# Byte transport keeps NULs and lone CRs intact at the Python boundary.
doc = Path(sys.argv[1]).read_bytes().decode("utf-8")
sys.stdout.write(cleandoc(doc=doc))
