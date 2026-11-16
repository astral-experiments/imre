from unittest import mock

mapping = {"state": "before"}
patcher = mock.patch.dict(mapping, {"state": "during"}, clear=True)
clear_dict = mock._clear_dict


def enter(target):
    print("enter", target is mapping, target["state"])
    clear_dict(in_dict=target)


def restore(target):
    print("restore", target is mapping, target["state"])
    clear_dict(in_dict=target)


with mock.patch.object(mock, "_clear_dict", enter):
    with patcher:
        mock._clear_dict = restore
print(mapping["state"])
