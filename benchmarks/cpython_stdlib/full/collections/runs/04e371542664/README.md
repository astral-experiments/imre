# Collections on the iterator repair

[Discovery](discovery.json) agrees with CPython for 15 collections IDs and the metadata of all 65 examples in four modes on the [resolved source](source.json). Executing the suite stops at the third prompt in Counter.elements: import math. See the [reproduction command](../230333bc121c/README.md).

The [ledger](evidence-linux.json) covers all 659 occurrences at O0 and O3: 7 fully native, 8 native bodies with CPython fixtures, 14 CPython-driven native operations, 4 without a collections operation, 3 reference skips, and 623 blocked. The [dict.fromkeys probe](evidence-fromkeys-linux.json) is separate. No IDs changed roles.

The [repair controls](repair-controls.json) match CPython for the original source generator (27 outputs), value struct (39), and named generator methods (10), in four modes.
