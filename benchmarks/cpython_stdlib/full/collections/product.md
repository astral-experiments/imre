# Integer product in `Counter.elements`

The fifth example in [the original `Counter.elements` docstring](upstream/Lib/collections/__init__.py#L645) multiplies the iterator from `Counter({2: 2, 3: 3, 17: 1})`. CPython prints `1836`. This requires a consuming operation over an integer iterator. No float, custom multiplication, or conversion to a list is used in the original example.

The [integer product fixture](../../../../tests/fixtures/iterables/math_product.py) compares complete process output with the pinned CPython interpreter at O0 and O3. It also exercises empty inputs, large integers, keyword-only `start`, argument evaluation order and errors after a zero factor. These controls do not count as the original doctest. That ID requires all five prompts, including the intervening `import math`, in order and in the same example namespace.
