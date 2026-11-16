from unittest.mock import patch


class Settings(dict):
    def copy(self):
        patcher.values = {"late": "value"}
        patcher.clear = True
        patcher.in_dict = replacement
        return dict(self)


settings = Settings(before="original")
replacement = {"other": "unchanged"}
patcher = patch.dict(settings, {"early": "value"})


def restore():
    print("restore", patcher.in_dict is replacement)
    original_restore()


with patcher as entered:
    print(entered is replacement, settings, replacement)
    original_restore = patcher._unpatch_dict
    patcher._unpatch_dict = restore
print(replacement)
