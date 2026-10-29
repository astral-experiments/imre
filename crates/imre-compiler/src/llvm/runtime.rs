//! Emit native declarations and calls from the same ABI contract.

use std::fmt;

use crate::contracts::{
    BorrowedIntOrError, OwnedIntOrError, OwnedWeakTargetOrError, RuntimeCall, RuntimeParam,
    StatusOrError,
};
use crate::ir::Type;

use super::{Emitter, Operand, Ownership, Value};

impl RuntimeParam {
    fn llvm(self) -> &'static str {
        match self {
            Self::Value(ty) => ty.llvm(),
            Self::NativeIndex => "i64",
            Self::WeakProxy => "ptr",
        }
    }

    fn format_operand(self, arg: &dyn RuntimeOperand, symbol: &str) -> String {
        let (actual, operand) = arg.runtime_operand();
        assert!(
            self == actual
                || matches!(
                    (self, actual),
                    (Self::WeakProxy, Self::Value(Type::WeakProxy(_)))
                ),
            "invalid argument to {symbol}: expected {self:?}, got {actual:?}",
        );
        format!("{} {operand}", self.llvm())
    }
}

pub(super) trait RuntimeOperand {
    fn runtime_operand(&self) -> (RuntimeParam, &str);
}

impl RuntimeOperand for Value {
    fn runtime_operand(&self) -> (RuntimeParam, &str) {
        (RuntimeParam::Value(self.ty), &self.operand)
    }
}

pub(super) struct NativeIndex(String);

impl RuntimeOperand for NativeIndex {
    fn runtime_operand(&self) -> (RuntimeParam, &str) {
        (RuntimeParam::NativeIndex, &self.0)
    }
}

pub(super) trait RuntimeReturn {
    type Output;

    fn llvm(&self) -> &'static str;
    fn emit(
        &self,
        emitter: &mut Emitter<'_, '_>,
        instruction: String,
        args: &[&dyn RuntimeOperand],
    ) -> Self::Output;
}

/// Register the transferred reference unless range analysis proves the word is immediate.
#[must_use]
pub(super) struct OwnedIntWord(String);

impl OwnedIntWord {
    pub(super) fn into_value(self, emitter: &mut Emitter<'_, '_>) -> Value {
        emitter.unpack_word(self.0, Type::Int)
    }

    pub(super) fn into_proven_small_word(self) -> String {
        self.0
    }
}

impl RuntimeReturn for OwnedIntOrError {
    type Output = OwnedIntWord;

    fn llvm(&self) -> &'static str {
        Type::Int.llvm()
    }

    fn emit(
        &self,
        emitter: &mut Emitter<'_, '_>,
        instruction: String,
        _: &[&dyn RuntimeOperand],
    ) -> OwnedIntWord {
        OwnedIntWord(emitter.checked_int_list_word(instruction))
    }
}

impl RuntimeReturn for BorrowedIntOrError {
    type Output = Operand;

    fn llvm(&self) -> &'static str {
        Type::Int.llvm()
    }

    fn emit(
        &self,
        emitter: &mut Emitter<'_, '_>,
        instruction: String,
        _: &[&dyn RuntimeOperand],
    ) -> Operand {
        let word = emitter.checked_int_list_word(instruction);
        Operand {
            value: emitter.decode_word(word, Type::Int),
            ownership: Ownership::Borrowed,
        }
    }
}

impl RuntimeReturn for OwnedWeakTargetOrError {
    type Output = Value;

    fn llvm(&self) -> &'static str {
        "ptr"
    }

    fn emit(
        &self,
        emitter: &mut Emitter<'_, '_>,
        instruction: String,
        args: &[&dyn RuntimeOperand],
    ) -> Value {
        let (RuntimeParam::Value(Type::WeakProxy(class)), _) = args[0].runtime_operand() else {
            unreachable!("validated weak proxy operand");
        };
        let operand = emitter.instruction(instruction);
        let failed = emitter.instruction(format!("icmp eq ptr {operand}, null"));
        emitter.check_failed(&failed);
        emitter.owned(Value {
            ty: Type::Class(class),
            operand,
            small_integer: false,
        })
    }
}

impl RuntimeReturn for StatusOrError {
    type Output = ();

    fn llvm(&self) -> &'static str {
        "i32"
    }

    fn emit(&self, emitter: &mut Emitter<'_, '_>, instruction: String, _: &[&dyn RuntimeOperand]) {
        emitter.checked_status(instruction);
    }
}

impl<R: RuntimeReturn, const N: usize> fmt::Display for RuntimeCall<R, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "declare {} @{}({})",
            self.result.llvm(),
            self.symbol,
            self.params.map(RuntimeParam::llvm).join(", ")
        )
    }
}

impl Emitter<'_, '_> {
    pub(super) fn native_index(&mut self, index: &Value) -> NativeIndex {
        assert_eq!(index.ty, Type::Int);
        NativeIndex(self.index_word(index, true))
    }

    pub(super) fn runtime_call<R: RuntimeReturn, const N: usize>(
        &mut self,
        call: &RuntimeCall<R, N>,
        args: [&dyn RuntimeOperand; N],
    ) -> R::Output {
        let operands = call
            .params
            .into_iter()
            .zip(args)
            .map(|(param, arg)| param.format_operand(arg, call.symbol))
            .collect::<Vec<_>>()
            .join(", ");
        let instruction = format!("call {} @{}({operands})", call.result.llvm(), call.symbol);
        call.result.emit(self, instruction, &args)
    }
}

#[cfg(test)]
mod tests {
    use crate::contracts::LIST_SET_INDEX;
    use crate::llvm::small;

    #[test]
    #[should_panic(expected = "invalid argument to imre_list_set_index")]
    fn native_setter_rejects_tagged_index() {
        LIST_SET_INDEX.params[1].format_operand(&small(1), LIST_SET_INDEX.symbol);
    }
}
