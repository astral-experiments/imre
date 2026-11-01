def current() -> str:
    return NAME


NAME = "@test"
ORIGINAL = current()
NAME = "file-" + NAME + "_tmp"
DEPENDENT = NAME + "-à"
if NAME.startswith("file-"):
    NAME = "new-" + NAME
INITIAL = "[" + NAME[0] + "]"
