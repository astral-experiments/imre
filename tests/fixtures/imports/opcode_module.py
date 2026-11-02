import _opcode_metadata
import dis
import opcode
from dis import opmap, opname

assert opcode.opmap is _opcode_metadata.opmap
assert opcode.opname[opcode.opmap["POP_TOP"]] == "POP_TOP"
assert opcode.stack_effect(opcode.opmap["POP_TOP"]) == -1
assert opcode.opmap["LOAD_CONST"] in opcode.hasconst
assert opcode._specializations is _opcode_metadata._specializations
assert opmap is dis.opmap is opcode.opmap
assert opname is dis.opname is opcode.opname
stop = dis.opmap["POP_TOP"]
jump = dis.opmap["JUMP_FORWARD"]
assert dis.findlabels(bytes((jump, 1, stop, 0, stop, 0))) == [4]
try:
    from dis import _missing_opcode_export
except ImportError:
    pass
else:
    raise AssertionError("dis exported a missing name")
print("opcode module OK")
