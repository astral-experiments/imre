"""Native assertion fixtures for the unchanged ChainMap constructor and truth tests."""


OPERATIONS = {
    "test_constructor": "Construct native ChainMaps and inspect the public mapping list",
    "test_bool": "Test the truth of ChainMaps with empty and nonempty dictionaries",
}


def fixture(method):
    if method == "test_bool":
        return """from collections import ChainMap

class NativeCase:
    def assertTrue(self, value: ChainMap[int, int]) -> None:
        if not value:
            raise AssertionError("ChainMap is false")

    def assertFalse(self, value: ChainMap[str, int]) -> None:
        if value:
            raise AssertionError("ChainMap is true")
"""
    if method != "test_constructor":
        raise ValueError(f"unsupported ChainMap assertion fixture: {method}")
    return """from collections import ChainMap

class NativeCase:
    def assertEqual(self, first: list[dict[int, int]], second: list[dict[int, int]]) -> None:
        if len(first) != len(second):
            raise AssertionError("mapping lists have different lengths")
        for index in range(len(first)):
            left, right = first[index], second[index]
            if len(left) != len(right):
                raise AssertionError("dictionaries have different lengths")
            for key in left:
                if key not in right or left[key] != right[key]:
                    raise AssertionError("different dictionary entry")
"""
