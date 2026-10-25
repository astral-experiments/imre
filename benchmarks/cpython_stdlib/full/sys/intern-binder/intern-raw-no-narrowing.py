"""Only empty/Latin-1 results are shared automatically across dynamic parent reads."""
import sys
from importlib.machinery import ModuleSpec

def inspect(s: str) -> None:
    source = s + ".nested"
    spec = ModuleSpec(source, None)
    first = spec.parent
    second = spec.parent
    print(repr(s))
    print(first == s)
    print(first is second)
    print(first is s)
    print(sys._is_interned(s))
    print(sys._is_interned(first))
    table: dict[int, int] = {}
    translated = first.translate(table)
    print(translated == first)
    print(translated is first)
    print(sys._is_interned(translated))
    canonical = sys.intern(first)
    print(canonical is s)
    print(canonical is first)
    print(sys._is_interned(first))
    print(sys.intern(second) is canonical)
    print(sys._is_interned(second))
    print(spec.parent is canonical)
    print(ModuleSpec(first, None, is_package=True).parent is first)
    if len(first) == 1:
        print(ord(first))
        print(sys.intern(chr(ord(first))) is canonical)

for s in ["", "a", "!", "_", ".", "é", "ÿ", "Ā", "λ", "界", "🙂", "aa", "_33", "long_identifier", "two words", "bleu_é", "not-a-name"]:
    inspect(s)

