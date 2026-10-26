# Sequence ABCs at 4435f75200d3

[Source and binaries](source.json). The [659 occurrences](evidence-linux.json) per mode comprise 614 blocked, 8 fully-native, 17 host-driven-native-operation, 1 mixed, 12 native-body-host-fixture, 4 no-product-operation, 3 reference-skip. The [separate dependency](evidence-fromkeys-linux.json) remains 0/1. The [O0](sequence-mixed-O0.json) and [O3](sequence-mixed-O3.json) native and host operations used CPython bodies, fixtures and assertions. Their negative controls fail the original assertions. [O0](abc-routines-O0.json) and [O3](abc-routines-O3.json) retain all 24 directly compiled ABC method attempts. [Four-mode comparisons](differential.json) add no occurrences; [fully native methods](verified-defaults.json) include CPython controls.

Build 4435f75200d3eaf33ab71b8f543348d46a48bbd5 and run with test-enabled CPython 3.14.5 from the repository root:

    "$CPYTHON" -B benchmarks/cpython_stdlib/full/collections/measure.py --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" --compiler-revision 4435f75200d3eaf33ab71b8f543348d46a48bbd5 --output "$NEW_RESULT_DIR"
