import logging

def inspect(name: str) -> None:
    result = logging.getLevelName(name)
    if isinstance(result, int):
        print(result)
        print(bool(result))
        print(result + 1)
    else:
        print(repr(result))
        print(bool(result))

inspect("INFO")
inspect("NOTSET")
inspect("unknown")
logging.addLevelName(20, "")
inspect("")
inspect("INFO")
print(repr(logging.getLevelName(20)))
large = 123456789012345678901234567890123
logging.addLevelName(levelName="BIG", level=large)
inspect("BIG")
logging.addLevelName(-large, "BIG")
inspect("BIG")
print(logging.getLevelName(large))
print(logging.getLevelName(-large))
a = logging.getLevelNamesMapping()
a["BIG"] = 55
print(logging.getLevelNamesMapping()["BIG"])
logging.basicConfig(level="BIG", format="%(levelname)s:%(message)s")
lg = logging.getLogger("q")
lg.setLevel("INFO")
lg.log(large, "large")
logging.basicConfig(level="no-level")
try:
    logging.basicConfig(level="no-level", force=True, format="%(message)s")
except ValueError as e:
    print(str(e))
logging.warning("uses-installed-handler")
try:
    lg.setLevel("Missing'\\\n")
except ValueError as e:
    print(str(e))
