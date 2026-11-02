import _opcode_metadata
import opcode

assert opcode.opmap is _opcode_metadata.opmap
assert opcode.opname[opcode.opmap["POP_TOP"]] == "POP_TOP"
assert opcode.stack_effect(opcode.opmap["POP_TOP"]) == -1
assert opcode.opmap["LOAD_CONST"] in opcode.hasconst
assert opcode._specializations is _opcode_metadata._specializations
print("opcode module OK")
