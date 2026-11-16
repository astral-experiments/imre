from unittest.mock import patch

settings = {"mode": "before"}
scope = patch.dict(settings, {"mode": "during"})
print(scope.__enter__() is settings, settings["mode"])
print(scope.__exit__(None, None, None), settings["mode"])
