//! Emit native declarations and calls from the same ABI contract.

use std::fmt;

use crate::contracts::{InfallibleVoid, OwnedIntOrError, RuntimeCall};
use crate::ir::Type;

use super::{Emitter, Value};

pub(super) trait RuntimeReturn {
    type Output;

    fn llvm(&self) -> &'static str;
    fn emit(&self, emitter: &mut Emitter<'_, '_>, instruction: String) -> Self::Output;
}

impl RuntimeReturn for OwnedIntOrError {
    type Output = String;

    fn llvm(&self) -> &'static str {
        Type::Int.llvm()
    }

    fn emit(&self, emitter: &mut Emitter<'_, '_>, instruction: String) -> String {
        emitter.checked_int_list_word(instruction)
    }
}

impl RuntimeReturn for InfallibleVoid {
    type Output = ();

    fn llvm(&self) -> &'static str {
        "void"
    }

    fn emit(&self, emitter: &mut Emitter<'_, '_>, instruction: String) {
        emitter.line(instruction);
    }
}

impl<R: RuntimeReturn, const N: usize> fmt::Display for RuntimeCall<R, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "declare {} @{}({})",
            self.result.llvm(),
            self.symbol,
            self.params.map(Type::llvm).join(", ")
        )
    }
}

impl Emitter<'_, '_> {
    pub(super) fn runtime_call<R: RuntimeReturn, const N: usize>(
        &mut self,
        call: &RuntimeCall<R, N>,
        args: [&Value; N],
    ) -> R::Output {
        let args = call
            .params
            .into_iter()
            .zip(args)
            .map(|(param, arg)| {
                assert_eq!(param, arg.ty, "invalid argument to {}", call.symbol);
                format!("{} {}", param.llvm(), arg.operand)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let instruction = format!("call {} @{}({args})", call.result.llvm(), call.symbol);
        call.result.emit(self, instruction)
    }
}
