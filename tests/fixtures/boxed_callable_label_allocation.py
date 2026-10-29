import sys
import time

sys.path.append("unused")


def answer_with_a_deliberately_long_qualified_name(value=41):
    return value


def answer_with_a_keyword_only_label(*, value=41):
    return value


targets = [list, dict, tuple, set, int, str, object, time.time, answer_with_a_deliberately_long_qualified_name, answer_with_a_keyword_only_label]
shape = int(sys.argv[1])
target = targets[shape]
marker = "arm callable label refusal"
kind = "ok"
try:
    result = target()
except MemoryError:
    kind = "MemoryError"
print("kind", kind)
retry = target()
print("preserved", target is targets[shape])
print("retry", type(retry).__name__, shape < 8 or retry == 41)
