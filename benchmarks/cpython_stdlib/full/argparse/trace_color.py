"""Trace the unchanged _colorize module under CPython's full test_argparse suite."""
import sys


CALLERS = {
    "_colorize.py": {"<module>"},
    "dataclasses.py": {"_process_class", "_make_annotate_function.<locals>.__annotate__"},
    "inspect.py": {"signature"},
    "annotationlib.py": {"get_annotations"},
}


def main():
    assert sys.version_info[:3] == (3, 14, 5) and "_colorize" not in sys.modules
    import opcode
    assert "_colorize" not in sys.modules
    phase = "import"
    calls, offsets, dataclass_calls, field_callers = {}, {}, {}, {}
    monitor = sys.monitoring
    tool = monitor.PROFILER_ID
    monitor.use_tool_id(tool, "argparse color reference")

    def entered(code, offset):
        if code.co_filename.endswith("/_colorize.py"):
            key = (phase, code.co_qualname)
            calls[key] = calls.get(key, 0) + 1
            monitor.set_local_events(tool, code, monitor.events.INSTRUCTION)
        if (phase == "import" and code.co_filename.endswith("/dataclasses.py")
                and code.co_qualname in {"field", "Field.__init__", "_get_field", "_process_class"}):
            key = code.co_qualname
            dataclass_calls[key] = dataclass_calls.get(key, 0) + 1

    def instruction(code, offset):
        offsets.setdefault((phase, code), set()).add(offset)
        # The pinned annotation has a one-byte name index. Record its callers
        # only when the Field load executes; some annotation formats stop earlier.
        if (code.co_qualname == "ThemeSection.__annotate__" and
                code.co_code[offset] == opcode.opmap["LOAD_FROM_DICT_OR_GLOBALS"] and
                code.co_names[code.co_code[offset + 1]] == "Field"):
            assert len(code.co_names) < 256
            frame = sys._getframe(1)
            chain = []
            while frame:
                filename = frame.f_code.co_filename.rsplit("/", 1)[-1]
                name = frame.f_code.co_qualname
                if name in CALLERS.get(filename, ()):
                    chain.append((filename, name, frame.f_lineno))
                frame = frame.f_back
            key = (phase, tuple(chain))
            field_callers[key] = field_callers.get(key, 0) + 1

    monitor.register_callback(tool, monitor.events.PY_START, entered)
    monitor.register_callback(tool, monitor.events.INSTRUCTION, instruction)
    monitor.set_events(tool, monitor.events.PY_START)
    import _colorize
    phase = "suite"
    import unittest
    from test import test_argparse
    suite = unittest.defaultTestLoader.loadTestsFromModule(test_argparse)
    result = unittest.TestResult()
    suite.run(result)
    assert result.testsRun == 1893 and result.wasSuccessful() and not result.skipped, (
        result.testsRun, result.errors, result.failures, result.skipped)
    assert not result.expectedFailures and not result.unexpectedSuccesses
    phase = "mapping_probe"
    from collections.abc import Mapping
    import dataclasses
    section = _colorize.default_theme.argparse
    assert isinstance(section, Mapping)
    names = list(section)
    assert len(section) == len(names) == 13
    assert section.get("usage") == section.usage and section.get("missing", "fallback") == "fallback"
    assert section.copy_with(usage="changed")["usage"] == "changed"
    assert all(_colorize.theme_no_color.argparse[name] == "" for name in names)
    assert all(isinstance(field, dataclasses.Field) and field.name == key
               for key, field in section.__dataclass_fields__.items())
    phase = "annotation_probe"
    import annotationlib
    annotations = annotationlib.get_annotations(_colorize.ThemeSection, format=annotationlib.Format.FORWARDREF)
    assert "__dataclass_fields__" in annotations
    monitor.set_events(tool, 0)
    for code in {code for _, code in offsets}:
        monitor.set_local_events(tool, code, 0)
    monitor.free_tool_id(tool)
    # These imports can load the color module themselves; delay them until the
    # monitored import and the original suite have finished.
    import dis
    import hashlib
    import json
    from pathlib import Path
    def blob(file):
        data = Path(file).read_bytes()
        return hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
    field, descriptors = {}, {}
    for (stage, code), visited in offsets.items():
        for instruction in dis.get_instructions(code):
            if instruction.offset not in visited:
                continue
            if instruction.argval == "Field":
                field.setdefault(stage, []).append([code.co_qualname, instruction.positions.lineno, instruction.opname])
            elif instruction.argval == "__dataclass_fields__" and instruction.opname == "LOAD_ATTR":
                descriptors.setdefault(stage, []).append([code.co_qualname, instruction.positions.lineno, instruction.opname])
    for records in (field, descriptors):
        for rows in records.values():
            rows.sort()
    phases = ("import", "suite", "mapping_probe", "annotation_probe")
    for stage in phases:
        field.setdefault(stage, [])
        descriptors.setdefault(stage, [])
    assert any(row[2] == "IMPORT_FROM" for row in field["import"]), (field, len(offsets))
    manifest = json.loads(Path(__file__).with_name("UPSTREAM.json").read_bytes())
    inputs = {"Lib/_colorize.py": blob(_colorize.__file__), "Lib/dataclasses.py": blob(dataclasses.__file__),
              "Lib/test/test_argparse.py": blob(test_argparse.__file__)}
    expected = manifest["files"] | manifest["reference_files"]
    assert all(expected[name] == digest for name, digest in inputs.items())
    report = {
        "implementation": "CPython", "version": "3.14.5", "cpython_commit": manifest["commit"],
        "input_blobs": inputs,
        "suite": {"module": "test.test_argparse", "tests": result.testsRun, "failures": 0, "errors": 0, "skips": 0, "native_tests": 0},
        "field_name_instructions": field, "field_dictionary_loads": descriptors,
        "field_annotation_callers": {stage: [{"loads": count, "callers": chain}
                                            for (s, chain), count in sorted(field_callers.items()) if s == stage]
                                    for stage in phases},
        "dataclasses_calls_during_colorize_import": dict(sorted(dataclass_calls.items())), "section_fields": names,
        "field_values": "dataclasses.Field for every Argparse key, in declaration order",
        "deferred_annotation": repr(annotations["__dataclass_fields__"]),
        "colorize_calls": {stage: {name: count for (s, name), count in sorted(calls.items())
                                   if s == stage and (name.startswith("ThemeSection.") or name == "get_theme")} for stage in phases},
    }
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
