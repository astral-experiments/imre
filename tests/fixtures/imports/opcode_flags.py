import _opcode as op
from _opcode import ENABLE_SPECIALIZATION_FT as specialize_ft

print(op.ENABLE_SPECIALIZATION)
print(specialize_ft)
if op.ENABLE_SPECIALIZATION or specialize_ft:
    print("specialized bytecode")
else:
    print("no bytecode specialization")
