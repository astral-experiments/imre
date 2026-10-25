"""Return native exception type and text for an original configuration operation."""

import json
import logging.config
import sys

try:
    if sys.argv[1] == "file":
        logging.config.fileConfig(sys.argv[2])
    else:
        logging.config.dictConfig(json.loads(input()))
except FileNotFoundError as error:
    print("FileNotFoundError")
    print(str(error))
except NotImplementedError as error:
    print("NotImplementedError")
    print(str(error))
except RuntimeError as error:
    print("RuntimeError")
    print(str(error))
except ValueError as error:
    print("ValueError")
    print(str(error))
else:
    print("OK")
