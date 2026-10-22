//! Ownership lowering and textual LLVM emission, with no C or interpreter layer.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::borrows::{self, FunctionBorrows};
use crate::effects::{self, ExceptionEffects};
use crate::ir::{
    Binary, Builtin, CallArgs, Compare, ExceptHandler, Expr, ExprKind, Function, InstanceType,
    Module, RegexCapture, Statement, Type,
};
use crate::ranges::{IntRange, RangeFacts};
use crate::types::Types;

mod callables;
mod conditionals;
mod integer_regions;
mod json;
mod literals;
mod paths;
mod processes;
mod records;
mod stdlib;
mod string_enums;
mod text;
mod truthiness;
mod unions;

impl Type {
    fn llvm(self) -> &'static str {
        match self {
            Self::Int | Self::StrEnum(_) | Self::Optional(_) | Self::Union(_) => "i64",
            Self::Float => "double",
            Self::Bool => "i1",
            Self::Str
            | Self::StrLiteral(_)
            | Self::Bytes
            | Self::ListInt
            | Self::Class(_)
            | Self::Callable(_)
            | Self::List(_)
            | Self::Dict(_)
            | Self::Set(_)
            | Self::Iterator
            | Self::Exception
            | Self::TimeoutExpired
            | Self::Path
            | Self::CompletedProcess
            | Self::TextIO
            | Self::RegexPattern
            | Self::RegexMatch
            | Self::BinaryIO
            | Self::Tuple(_) => "ptr",
            Self::None => "i8",
        }
    }

    fn initial(self) -> &'static str {
        match self {
            Self::Int => "1",
            Self::Float => "0.0",
            Self::Str
            | Self::StrLiteral(_)
            | Self::Bytes
            | Self::ListInt
            | Self::Class(_)
            | Self::Callable(_)
            | Self::List(_)
            | Self::Dict(_)
            | Self::Set(_)
            | Self::Iterator
            | Self::Exception
            | Self::TimeoutExpired
            | Self::Path
            | Self::CompletedProcess
            | Self::TextIO
            | Self::RegexPattern
            | Self::RegexMatch
            | Self::BinaryIO
            | Self::Tuple(_) => "null",
            Self::Bool | Self::None | Self::StrEnum(_) | Self::Optional(_) | Self::Union(_) => "0",
        }
    }
}

#[derive(Clone)]
struct Value {
    ty: Type,
    operand: String,
    small_integer: bool,
}

#[derive(Clone, Copy)]
enum Ownership {
    Owned,
    Borrowed,
}

/// A call or operator input may borrow a local root instead of acquiring a temporary.
struct Operand {
    value: Value,
    ownership: Ownership,
}

/// Loop-private views of unchanged incoming lists; no field owns a reference.
#[derive(Clone)]
struct LengthSnapshot {
    receiver: Value,
    length: Value,
    data: Option<String>,
}

/// Finally routing chooses the eventual transfer before it reaches either
/// the break exit or the distinct normal-exhaustion/else entry.
#[derive(Clone)]
struct LoopTarget {
    end: String,
    exhausted: String,
    next: String,
    cleanup_depth: usize,
}

/// Lexical cleanup obligations are separate from expression-owned SSA values.
/// A handler can retain user locals while releasing temporary scopes exited by
/// an error; finally bodies may replace the completion currently being routed.
#[derive(Clone)]
enum Cleanup {
    Handler(String),
    Finally {
        body: Vec<Statement>,
        loops: Vec<LoopTarget>,
    },
    Slots(Vec<usize>),
    Active {
        previous: Value,
        name: Option<usize>,
    },
    Completion(Value),
}

enum Completion {
    Return(Value),
    Jump { label: String, depth: usize },
    Error,
}

pub(crate) fn emit(module: &Module, ranges: &RangeFacts<'_>, sanitize: bool) -> String {
    let borrows = borrows::analyze(module);
    let effects = effects::analyze(module);
    let inline_dispatch = string_enums::inline_dispatch(module, &effects);
    let mut emitter = Emitter {
        module,
        code: String::new(),
        entry_allocas: String::new(),
        strings: Vec::new(),
        string_literals: BTreeMap::new(),
        regex_literals: BTreeMap::new(),
        enum_members: Vec::new(),
        literal_globals: literal_globals(module),
        next_value: 0,
        next_label: 0,
        current: "entry".to_owned(),
        terminated: false,
        loops: Vec::new(),
        locals: Vec::new(),
        borrowed_params: Vec::new(),
        borrowed_locals: Vec::new(),
        rooted_locals: Vec::new(),
        length_roots: Vec::new(),
        length_snapshots: Vec::new(),
        expression_rebound: Vec::new(),
        small_locals: Vec::new(),
        temporaries: Vec::new(),
        cleanups: Vec::new(),
        return_exit: String::new(),
        error_exit: String::new(),
        sanitize,
        ranges,
        effects,
        inline_dispatch,
        callable_targets: crate::callables::Targets::analyze(module),
        integer_regions: integer_regions::analyze(module),
        uses_sys_argv: module.uses_sys_argv(),
    };
    emitter.prepare_enum_members();
    for (id, function) in module.functions.iter().enumerate() {
        emitter.function(function, &format!("fn_{id}"), false, &borrows[id]);
    }
    emitter.function(&module.entry, "main", true, borrows.last().unwrap());
    let mut output = String::from(
        "; Generated by Relar. Runtime arguments borrow; results transfer ownership.\n",
    );
    output.push_str(DECLARATIONS);
    output.push_str(&record_layouts(module));
    output.push_str(&json::schemas(module));
    output.push_str(&callables::adapters(module, sanitize));
    output.push_str(&integer_helpers());
    output.push_str(&nullable_pointer_helpers());
    for (slot, ty) in module.globals.iter().enumerate() {
        writeln!(
            output,
            "@module_value_{slot} = internal global {} {}",
            ty.llvm(),
            ty.initial()
        )
        .unwrap();
    }
    for (id, bytes) in emitter.strings.iter().enumerate() {
        write!(
            output,
            "@str_{id} = private unnamed_addr constant [{} x i8] c\"",
            bytes.len()
        )
        .unwrap();
        for byte in bytes {
            write!(output, "\\{byte:02X}").unwrap();
        }
        output.push_str("\"\n");
    }
    output.push_str(&emitter.enum_table());
    output.push_str(&emitter.literal_pool());
    output.push_str(&emitter.code);
    // Layout hints never replace the representation, overflow, or error guards.
    output.push_str("\n!0 = !{!\"branch_weights\", i32 1, i32 2000}\n");
    output.push_str("!1 = !{!\"branch_weights\", i32 2000, i32 1}\n");
    output
}

struct Emitter<'a, 'module> {
    module: &'a Module,
    code: String,
    entry_allocas: String,
    strings: Vec<Vec<u8>>,
    string_literals: BTreeMap<Vec<u8>, usize>,
    regex_literals: BTreeMap<Vec<u8>, usize>,
    enum_members: Vec<[usize; 3]>,
    literal_globals: Vec<Option<String>>,
    next_value: usize,
    next_label: usize,
    current: String,
    terminated: bool,
    loops: Vec<LoopTarget>,
    locals: Vec<Type>,
    borrowed_params: Vec<bool>,
    borrowed_locals: Vec<bool>,
    rooted_locals: Vec<bool>,
    length_roots: Vec<Option<usize>>,
    length_snapshots: Vec<Option<LengthSnapshot>>,
    expression_rebound: Vec<bool>,
    small_locals: Vec<bool>,
    temporaries: Vec<Value>,
    cleanups: Vec<Cleanup>,
    return_exit: String,
    error_exit: String,
    sanitize: bool,
    ranges: &'a RangeFacts<'module>,
    effects: ExceptionEffects,
    inline_dispatch: Vec<bool>,
    callable_targets: crate::callables::Targets,
    integer_regions: Vec<Option<integer_regions::Summary>>,
    uses_sys_argv: bool,
}

impl Emitter<'_, '_> {
    fn line(&mut self, line: impl AsRef<str>) {
        writeln!(self.code, "  {}", line.as_ref()).unwrap();
    }

    fn instruction(&mut self, instruction: impl AsRef<str>) -> String {
        let result = format!("%v{}", self.next_value);
        self.next_value += 1;
        self.line(format!("{result} = {}", instruction.as_ref()));
        result
    }

    /// Gives each emitted operation independent scratch with function lifetime,
    /// including operations inside loops and duplicated finally cleanup paths.
    fn entry_alloca(&mut self, ty: impl AsRef<str>) -> String {
        let result = format!("%scratch{}", self.next_value);
        self.next_value += 1;
        writeln!(self.entry_allocas, "  {result} = alloca {}", ty.as_ref()).unwrap();
        result
    }

    /// Checks the pending error before exposing a fallible call's result to
    /// word conversion, ownership registration or subsequent source effects.
    fn checked_instruction(&mut self, instruction: impl AsRef<str>) -> String {
        let result = self.instruction(instruction);
        self.check_error();
        result
    }

    fn checked_line(&mut self, line: impl AsRef<str>) {
        self.line(line);
        self.check_error();
    }

    /// Integer-list reads return raw zero only after installing an error; every
    /// canonical Int, including numeric zero, has a nonzero word. Pending must
    /// be clear on entry, as guaranteed by immediate checks and by taking an
    /// error before entering handlers/finally. Do not decode or own the sentinel.
    fn checked_int_list_word(&mut self, instruction: impl AsRef<str>) -> String {
        let word = self.instruction(instruction);
        let failed = self.instruction(format!("icmp eq i64 {word}, 0"));
        self.check_failed(&failed);
        word
    }

    /// Integer-list writes return one on success and zero with a pending error
    /// on failure. The same clear-pending entry contract as reads applies, and
    /// existing expression owners remain live until this status is checked.
    fn checked_int_list_status(&mut self, instruction: impl AsRef<str>) {
        let status = self.instruction(instruction);
        let failed = self.instruction(format!("icmp eq i8 {status}, 0"));
        self.check_failed(&failed);
    }

    fn label(&mut self) -> String {
        let result = format!("b{}", self.next_label);
        self.next_label += 1;
        result
    }

    fn start(&mut self, label: &str) {
        writeln!(self.code, "{label}:").unwrap();
        self.current = label.to_owned();
        self.terminated = false;
    }

    fn jump(&mut self, label: &str) {
        self.line(format!("br label %{label}"));
        self.terminated = true;
    }

    fn branch(&mut self, condition: &str, yes: &str, no: &str) {
        self.line(format!("br i1 {condition}, label %{yes}, label %{no}"));
        self.terminated = true;
    }

    fn call(&mut self, name: &str, returns: Type, args: &[&Value]) -> Value {
        self.call_with_effect(name, returns, args, fallible(name), false)
    }

    /// Uses only the final IR's conservative callee summary. Argument evaluation
    /// has already performed its own checks and rooted every completed value.
    fn user_call(
        &mut self,
        function: usize,
        returns: Type,
        args: &[&Value],
        enum_match: bool,
    ) -> Value {
        self.call_with_effect(
            &format!("fn_{function}"),
            returns,
            args,
            self.effects.may_raise(function),
            enum_match && self.inline_dispatch[function],
        )
    }

    /// Captures in source order, preserving earlier owners across later argument
    /// effects and errors. Parameter reordering happens only after this step.
    fn call_operands(&mut self, args: &CallArgs) -> Vec<Operand> {
        args.values.iter().map(|arg| self.operand(arg)).collect()
    }

    fn call_with_effect(
        &mut self,
        name: &str,
        returns: Type,
        args: &[&Value],
        may_raise: bool,
        inline: bool,
    ) -> Value {
        let args = args
            .iter()
            .map(|value| format!("{} {}", value.ty.llvm(), value.operand))
            .collect::<Vec<_>>()
            .join(", ");
        let attribute = if inline { " alwaysinline" } else { "" };
        let instruction = format!("call {} @{name}({args}){attribute}", returns.llvm());
        let operand = if may_raise {
            self.checked_instruction(instruction)
        } else {
            self.instruction(instruction)
        };
        let value = Value {
            ty: returns,
            operand,
            small_integer: false,
        };
        self.owned(value)
    }

    fn void_call(&mut self, name: &str, args: &[&Value]) {
        let args = args
            .iter()
            .map(|value| format!("{} {}", value.ty.llvm(), value.operand))
            .collect::<Vec<_>>()
            .join(", ");
        let instruction = format!("call void @{name}({args})");
        if fallible(name) {
            self.checked_line(instruction);
        } else {
            self.line(instruction);
        }
    }

    fn retain(&mut self, value: &Value) {
        self.reference(value, "retain");
    }

    fn release(&mut self, value: &Value) {
        self.forget(value);
        self.reference(value, "release");
    }

    /// Canonical exact integers are nonzero, including cleared locals whose
    /// default is tagged zero. Optional payloads retain their nullable helper;
    /// replacement cleanup handles uninitialized record words separately.
    fn reference(&mut self, value: &Value, operation: &str) {
        if value.small_integer {
            return;
        }
        let ty = self.module.types.payload(value.ty).unwrap_or(value.ty);
        if ty == Type::Float {
            if value.ty != Type::Float {
                self.void_call(&format!("relar_float_box_{operation}"), &[value]);
            }
            return;
        }
        let family = match ty {
            Type::Int => "int",
            Type::Union(_) => "union",
            Type::Str | Type::StrLiteral(_) => "str",
            Type::Bytes => "bytes",
            Type::ListInt => "list",
            Type::Class(_) | Type::Tuple(_) | Type::Callable(_) | Type::CompletedProcess => {
                "record"
            }
            Type::List(_)
            | Type::Dict(_)
            | Type::Set(_)
            | Type::Iterator
            | Type::Exception
            | Type::TimeoutExpired
            | Type::Path
            | Type::TextIO
            | Type::RegexPattern
            | Type::RegexMatch
            | Type::BinaryIO => "object",
            Type::Float | Type::Bool | Type::StrEnum(_) | Type::None => return,
            Type::Optional(_) => unreachable!("optional types are flattened"),
        };
        let name = if value.ty == Type::Int {
            format!("{operation}_int_nonzero")
        } else if ty == Type::Int {
            format!("{operation}_int")
        } else if matches!(family, "str" | "object" | "record")
            && (operation == "release" || value.ty != ty)
        {
            format!("{family}_{operation}_if_present")
        } else {
            format!("relar_{family}_{operation}")
        };
        let payload = if ty == value.ty {
            value.clone()
        } else {
            self.decode_word(value.operand.clone(), ty)
        };
        self.void_call(&name, &[&payload]);
    }

    /// Registers one owner only after a call's error check. Fallible universal
    /// word getters may return zero on failure, which is not a valid owned Int.
    fn owned(&mut self, value: Value) -> Value {
        if value.ty != Type::Float
            && !matches!(
                self.module.types.payload(value.ty).unwrap_or(value.ty),
                Type::Bool | Type::None | Type::StrEnum(_)
            )
            && !value.small_integer
            && !self
                .temporaries
                .iter()
                .any(|temporary| temporary.operand == value.operand)
        {
            self.temporaries.push(value.clone());
        }
        value
    }

    fn forget(&mut self, value: &Value) {
        self.temporaries
            .retain(|temporary| temporary.operand != value.operand);
    }

    /// Other fallible calls, including nullable/scalar generic getters, retain
    /// the pending-flag ABI because zero can also be a valid successful result.
    fn check_error(&mut self) {
        let pending = self.instruction("call i8 @relar_error_pending()");
        let pending = self.instruction(format!("icmp ne i8 {pending}, 0"));
        self.check_failed(&pending);
    }

    /// Both error-detection ABIs share exactly the same ownership routing. The
    /// error edge consumes only owners produced before the failed call; success
    /// restores those SSA owners before inspecting a result or out-parameter.
    /// Branch weights guide layout/inlining without assuming errors impossible.
    fn check_failed(&mut self, failed: &str) {
        let (error, success) = (self.label(), self.label());
        self.line(format!(
            "br i1 {failed}, label %{error}, label %{success}, !prof !0"
        ));
        self.terminated = true;
        let temporaries = self.temporaries.clone();
        self.start(&error);
        for value in temporaries.iter().rev() {
            self.release(value);
        }
        self.route(Completion::Error);
        self.start(&success);
        self.temporaries = temporaries;
    }

    /// Borrows direct locals only when no expression can replace their slot.
    /// Earlier operands of a call or operator must survive a later `:=` write.
    /// Stable caller roots also protect projections in edge-preserving functions.
    fn operand(&mut self, expression: &Expr) -> Operand {
        if let ExprKind::Builtin(Builtin::GlobalGet(slot), _) = &expression.kind {
            // A checked global is published once and remains rooted until main exits.
            Operand {
                value: self.borrow_global(*slot, expression.ty),
                ownership: Ownership::Borrowed,
            }
        } else if let ExprKind::Local(slot) = &expression.kind
            && !self.expression_rebound[*slot]
        {
            let mut value = self.borrow_slot(*slot);
            value.small_integer |= self.ranges.integer(expression).is_some();
            Operand {
                value,
                ownership: Ownership::Borrowed,
            }
        } else if self.can_borrow_projection(expression) {
            Operand {
                value: self.borrow_projection(expression),
                ownership: Ownership::Borrowed,
            }
        } else {
            Operand {
                value: self.expr(expression),
                ownership: Ownership::Owned,
            }
        }
    }

    fn borrow_global(&mut self, slot: usize, ty: Type) -> Value {
        Value {
            ty,
            operand: self.instruction(format!("load {}, ptr @module_value_{slot}", ty.llvm())),
            small_integer: false,
        }
    }

    /// A field, length, or truth read cannot run user code or replace a local.
    /// Keep each field along the path borrowed until this immediate read ends.
    fn immediate_receiver(&mut self, expression: &Expr) -> Operand {
        if let Some(value) = self.immediate_projection(expression) {
            Operand {
                value,
                ownership: Ownership::Borrowed,
            }
        } else {
            self.operand(expression)
        }
    }

    fn immediate_projection(&mut self, expression: &Expr) -> Option<Value> {
        Some(match &expression.kind {
            ExprKind::Local(slot) => self.borrow_slot(*slot),
            ExprKind::Call(..) => self.selected_list_field(expression)?,
            ExprKind::Unwrap(inner)
                if let ExprKind::Local(slot) = inner.kind
                    && !matches!(expression.ty, Type::Bool | Type::Float) =>
            {
                let word = self.borrow_slot(slot).operand;
                self.decode_word(word, expression.ty)
            }
            ExprKind::Field(receiver, field) => {
                let receiver = self.immediate_projection(receiver)?;
                let pointer = self.field_pointer(&receiver, *field);
                self.field_value(&pointer, expression.ty)
            }
            _ => return None,
        })
    }

    /// Select a field address before loading its list. The receiver is a local
    /// root and the checked unboxed predicate cannot run user code.
    fn selected_list_field(&mut self, expression: &Expr) -> Option<Value> {
        let getter = borrows::getters::selected(expression, &self.module.functions)?;
        let test = self.expr(&getter.test);
        let record = self.borrow_slot(getter.root);
        let yes = self.field_pointer(&record, getter.yes);
        let no = self.field_pointer(&record, getter.no);
        let field = self.instruction(format!("select i1 {}, ptr {yes}, ptr {no}", test.operand));
        Some(self.field_value(&field, expression.ty))
    }

    fn release_operand(&mut self, operand: &Operand) {
        if matches!(operand.ownership, Ownership::Owned) {
            self.release(&operand.value);
        }
    }

    /// Converts an identity operation's borrowed input into its owned result.
    fn own_operand(&mut self, operand: Operand) -> Value {
        if matches!(operand.ownership, Ownership::Borrowed) {
            self.retain(&operand.value);
        }
        self.owned(operand.value)
    }

    /// Acquires an owned value for assignment or return, which may outlive or
    /// replace the source slot.
    fn load(&mut self, slot: usize, small_integer: bool) -> Value {
        let mut value = self.borrow_slot(slot);
        value.small_integer |= small_integer;
        self.retain(&value);
        self.owned(value)
    }

    fn borrow_slot(&mut self, slot: usize) -> Value {
        let ty = self.locals[slot];
        Value {
            ty,
            operand: self.instruction(format!("load {}, ptr %local{slot}", ty.llvm())),
            small_integer: self.small_locals[slot],
        }
    }

    fn can_borrow_projection(&self, expression: &Expr) -> bool {
        borrows::projection_root(expression, &self.module.types, &self.module.functions)
            .is_some_and(|slot| self.rooted_locals[slot])
    }

    /// Reads an object word protected by a stable caller root, never registering
    /// a temporary owner or exposing a pointer into movable sequence storage.
    fn borrow_projection(&mut self, expression: &Expr) -> Value {
        debug_assert!(self.can_borrow_projection(expression));
        let mut value = match &expression.kind {
            ExprKind::Local(slot) => self.borrow_slot(*slot),
            ExprKind::Call(..) => self
                .selected_list_field(expression)
                .expect("proven list getter"),
            ExprKind::Field(receiver, field) => {
                let receiver = self.borrow_projection(receiver);
                let pointer = self.field_pointer(&receiver, *field);
                self.field_value(&pointer, expression.ty)
            }
            ExprKind::Builtin(Builtin::SequenceGetInBounds, args) => {
                let ExprKind::Local(receiver_slot) = args[0].kind else {
                    unreachable!("proven sequence receivers are private locals")
                };
                let receiver = self.borrow_projection(&args[0]);
                let index = match args[1].kind {
                    ExprKind::Local(slot) => self.borrow_slot(slot),
                    ExprKind::Int(_) => self.expr(&args[1]),
                    _ => unreachable!("proven sequence indices are private cursors or constants"),
                };
                self.borrow_sequence_element(receiver_slot, &receiver, &index, expression.ty)
            }
            ExprKind::Present(inner) => {
                let value = self.borrow_projection(inner);
                let word = self.word(&value);
                Value {
                    ty: expression.ty,
                    operand: if value.ty == Type::Bool {
                        self.instruction(format!("add i64 {word}, 1"))
                    } else {
                        word
                    },
                    small_integer: false,
                }
            }
            ExprKind::Unwrap(inner) => {
                let value = self.borrow_projection(inner);
                let word = if expression.ty == Type::Bool {
                    self.instruction(format!("sub i64 {}, 1", value.operand))
                } else {
                    value.operand
                };
                self.decode_word(word, expression.ty)
            }
            ExprKind::Scope { body, value, clear } => {
                self.cleanups.push(Cleanup::Slots(clear.clone()));
                self.block(body);
                let value = self.borrow_projection(value);
                self.cleanups.pop();
                self.clear_slots(clear);
                value
            }
            _ => unreachable!("borrowed projections were checked"),
        };
        value.small_integer |= self.ranges.integer(expression).is_some();
        value
    }

    /// The adjacent live-length guard already checked this private cursor.
    /// Callers either retain immediately or prove the enclosing root stable.
    fn borrow_sequence_element(
        &mut self,
        slot: usize,
        receiver: &Value,
        index: &Value,
        ty: Type,
    ) -> Value {
        debug_assert!(matches!(receiver.ty, Type::List(_)));
        debug_assert_eq!(self.module.types.element(receiver.ty), Some(ty));
        let mut index = index.clone();
        index.small_integer = true;
        let index = self.index_word(&index, true);
        let word = self.proven_sequence_word(slot, receiver, &index);
        self.decode_word(word, ty)
    }

    /// A frozen view is usable only for its current receiver identity. Clear or
    /// Move can reset identity-only aliases, so unequal values keep the ordinary
    /// proven read. The adjacent bounds proof dominates either element load.
    fn proven_sequence_word(&mut self, slot: usize, receiver: &Value, index: &str) -> String {
        let snapshot = self.length_roots[slot]
            .and_then(|root| self.length_snapshots[root].clone().map(|view| (root, view)))
            .filter(|(_, view)| view.data.is_some());
        let Some((root, view)) = snapshot else {
            return self.instruction(format!(
                "call i64 @relar_seq_borrow_index_proven_fast(ptr {}, i64 {index})",
                receiver.operand
            ));
        };
        let data = view.data.unwrap();
        if root == slot {
            let element = self.instruction(format!(
                "getelementptr inbounds i64, ptr {data}, i64 {index}"
            ));
            return self.instruction(format!("load i64, ptr {element}"));
        }
        let same = self.instruction(format!(
            "icmp eq ptr {}, {}",
            receiver.operand, view.receiver.operand
        ));
        let (cached, fallback, end) = (self.label(), self.label(), self.label());
        self.branch(&same, &cached, &fallback);
        self.start(&cached);
        let element = self.instruction(format!(
            "getelementptr inbounds i64, ptr {data}, i64 {index}"
        ));
        let word = self.instruction(format!("load i64, ptr {element}"));
        self.jump(&end);
        self.start(&fallback);
        let ordinary = self.instruction(format!(
            "call i64 @relar_seq_borrow_index_proven_fast(ptr {}, i64 {index})",
            receiver.operand
        ));
        self.jump(&end);
        self.start(&end);
        self.instruction(format!(
            "phi i64 [{word}, %{cached}], [{ordinary}, %{fallback}]"
        ))
    }

    /// A borrowed slot has no independent owner to release or transfer.
    fn store_borrowed(&mut self, slot: usize, value: &Value) {
        debug_assert!(self.borrowed_locals[slot]);
        debug_assert_eq!(self.locals[slot], value.ty);
        self.line(format!(
            "store {} {}, ptr %local{slot}",
            value.ty.llvm(),
            value.operand
        ));
    }

    fn reset_borrowed(&mut self, slot: usize) {
        debug_assert!(self.borrowed_locals[slot]);
        let ty = self.locals[slot];
        self.line(format!(
            "store {} {}, ptr %local{slot}",
            ty.llvm(),
            ty.initial()
        ));
    }

    /// Hoists nonallocating lengths and required generic-sequence buffer views
    /// of unchanged incoming lists in a transitively readonly function. Nested
    /// loops share dominating snapshots; the caller restores the previous cache
    /// after emitting this loop. Data pointers never enter language values.
    fn snapshot_loop_lengths(&mut self, statement: &Statement) -> Vec<Option<LengthSnapshot>> {
        let previous = self.length_snapshots.clone();
        let mut roots = vec![(false, false); self.locals.len()];
        crate::ir::walk_expressions(std::slice::from_ref(statement), &mut |expression| {
            if let ExprKind::Builtin(
                operation @ (Builtin::Len | Builtin::SequenceGetInBounds),
                args,
            ) = &expression.kind
                && let ExprKind::Local(slot) = args[0].kind
                && let Some(root) = self.length_roots[slot]
            {
                roots[root].0 = true;
                roots[root].1 |= matches!(operation, Builtin::SequenceGetInBounds);
            }
        });
        for (root, (needed, data_needed)) in roots.into_iter().enumerate() {
            if needed && self.length_snapshots[root].is_none() {
                let receiver = self.borrow_slot(root);
                let length = self.list_length(&receiver);
                self.length_snapshots[root] = Some(LengthSnapshot {
                    receiver,
                    length,
                    data: None,
                });
            }
            if data_needed {
                let snapshot = self.length_snapshots[root].as_ref().unwrap();
                if snapshot.data.is_none() {
                    debug_assert!(matches!(snapshot.receiver.ty, Type::List(_)));
                    let data = self.instruction(format!(
                        "call ptr @relar_seq_data_readonly_fast(ptr {})",
                        snapshot.receiver.operand
                    ));
                    self.length_snapshots[root].as_mut().unwrap().data = Some(data);
                }
            }
        }
        previous
    }

    /// Exact list storage bounds its length below the tagged-small maximum.
    /// Unlike bytes or strings, this read cannot allocate a BigInt owner.
    fn list_length(&mut self, receiver: &Value) -> Value {
        let helper = match receiver.ty {
            Type::ListInt => "relar_list_len",
            Type::List(_) => "relar_seq_len_fast",
            _ => unreachable!("only exact lists have nonallocating snapshots"),
        };
        Value {
            ty: Type::Int,
            operand: self.instruction(format!("call i64 @{helper}(ptr {})", receiver.operand)),
            small_integer: true,
        }
    }

    /// Identity-only definitions do not exclude Clear/Move resets. A current
    /// pointer guard preserves the original length read on every unequal value,
    /// including a cleared alias; no hidden-slot liveness assumption is needed.
    fn snapshot_length(&mut self, slot: usize, receiver: &Value) -> Option<Value> {
        let root = self.length_roots[slot]?;
        let snapshot = self.length_snapshots[root].clone()?;
        if slot == root {
            return Some(snapshot.length);
        }
        let same = self.instruction(format!(
            "icmp eq ptr {}, {}",
            receiver.operand, snapshot.receiver.operand
        ));
        let (fallback, end) = (self.label(), self.label());
        let before = self.current.clone();
        self.branch(&same, &end, &fallback);
        self.start(&fallback);
        let ordinary = self.list_length(receiver);
        self.jump(&end);
        self.start(&end);
        Some(Value {
            ty: Type::Int,
            operand: self.instruction(format!(
                "phi i64 [{}, %{before}], [{}, %{fallback}]",
                snapshot.length.operand, ordinary.operand
            )),
            small_integer: true,
        })
    }

    /// Transfers a local's ownership to the returned value and leaves a valid
    /// empty/immediate value behind for subsequent reassignment or cleanup.
    fn take_slot(&mut self, slot: usize) -> Value {
        debug_assert!(!self.borrowed_params[slot]);
        debug_assert!(!self.borrowed_locals[slot]);
        let value = self.borrow_slot(slot);
        self.line(format!(
            "store {} {}, ptr %local{slot}",
            value.ty.llvm(),
            value.ty.initial()
        ));
        self.owned(value)
    }

    /// Publishes the new owned value before releasing the replaced reference.
    fn store(&mut self, slot: usize, value: &Value) {
        self.store_with_previous_small(slot, value, false);
    }

    /// A preceding same-slot read may prove the replaced value canonical-small
    /// at this assignment, even when other paths store BigInts in the slot.
    /// The caller must exclude every intervening write and cleanup, including
    /// expression-owned scopes and walrus rebinding. This fact applies only
    /// after successful RHS evaluation; exception cleanup keeps its own owners.
    fn store_with_previous_small(&mut self, slot: usize, value: &Value, previous_small: bool) {
        debug_assert!(!self.borrowed_params[slot]);
        debug_assert!(!self.borrowed_locals[slot]);
        debug_assert!(!previous_small || self.locals[slot] == Type::Int);
        let old = (!previous_small).then(|| self.borrow_slot(slot));
        self.line(format!(
            "store {} {}, ptr %local{slot}",
            value.ty.llvm(),
            value.operand
        ));
        self.forget(value);
        if let Some(old) = old {
            self.release(&old);
        }
    }

    /// Transfers an evaluated left operand's proof to the old assignment slot.
    /// A user call cannot rebind a caller's immutable integer local, but hidden
    /// expression statements and binding expressions can replace its owner.
    fn assignment_replaces_small(&self, slot: usize, expression: &Expr) -> bool {
        if self.locals[slot] != Type::Int || self.expression_rebound[slot] {
            return false;
        }
        let ExprKind::Binary(_, left, right) = &expression.kind else {
            return false;
        };
        let same_local = match &left.kind {
            ExprKind::Local(source) => *source == slot,
            ExprKind::Builtin(Builtin::IntBounded { .. }, args) => {
                matches!(args.as_slice(), [Expr { kind: ExprKind::Local(source), .. }] if *source == slot)
            }
            _ => false,
        };
        same_local
            && self.ranges.integer(left).is_some()
            && expression_preserves_local(right, slot, self.locals.len())
    }

    fn cleanup(&mut self) {
        for slot in 0..self.locals.len() {
            if self.borrowed_params[slot] || self.borrowed_locals[slot] {
                continue;
            }
            let value = self.take_slot(slot);
            self.release(&value);
        }
    }

    /// Releases initialized module owners and harmless defaults on either exit.
    fn cleanup_globals(&mut self) {
        for slot in (0..self.module.globals.len()).rev() {
            let ty = self.module.globals[slot];
            let value = Value {
                ty,
                operand: self.instruction(format!("load {}, ptr @module_value_{slot}", ty.llvm())),
                small_integer: false,
            };
            self.line(format!(
                "store {} {}, ptr @module_value_{slot}",
                ty.llvm(),
                ty.initial()
            ));
            self.release(&value);
        }
    }

    fn clear_slots(&mut self, slots: &[usize]) {
        for &slot in slots {
            if self.borrowed_locals[slot] {
                self.reset_borrowed(slot);
                continue;
            }
            let value = self.take_slot(slot);
            self.release(&value);
        }
    }

    /// Restores the dynamically active exception independently of the pending
    /// error being propagated. Handler names are deleted even when rebound.
    fn restore_active(&mut self, previous: &Value, name: Option<usize>) {
        if let Some(name) = name {
            self.clear_slots(&[name]);
        }
        let current = self.call("relar_exception_swap_active", Type::Exception, &[previous]);
        self.release(&current);
        self.release(previous);
    }

    /// Emits one completion while preserving the lexical stack for other CFG
    /// edges. A finally runs with its own lexical loop targets, which can differ
    /// from the loops surrounding the instruction that initiated unwinding.
    fn route(&mut self, completion: Completion) {
        let cleanups = self.cleanups.clone();
        let loops = self.loops.clone();
        self.unwind(completion);
        self.cleanups = cleanups;
        self.loops = loops;
    }

    fn unwind(&mut self, completion: Completion) {
        let stop = match &completion {
            Completion::Jump { depth, .. } => *depth,
            Completion::Return(_) | Completion::Error => 0,
        };
        while self.cleanups.len() > stop {
            match self.cleanups.pop().unwrap() {
                Cleanup::Handler(label) => {
                    if matches!(completion, Completion::Error) {
                        self.jump(&label);
                        return;
                    }
                }
                Cleanup::Slots(slots) => self.clear_slots(&slots),
                Cleanup::Active { previous, name } => self.restore_active(&previous, name),
                Cleanup::Completion(value) => self.release(&value),
                Cleanup::Finally { body, loops } => {
                    let depth = self.cleanups.len();
                    let error = if matches!(completion, Completion::Error) {
                        let error = self.call("relar_error_take", Type::Exception, &[]);
                        let previous =
                            self.call("relar_exception_swap_active", Type::Exception, &[&error]);
                        self.forget(&error);
                        self.forget(&previous);
                        self.cleanups.push(Cleanup::Completion(error.clone()));
                        self.cleanups.push(Cleanup::Active {
                            previous: previous.clone(),
                            name: None,
                        });
                        Some((error, previous))
                    } else {
                        if let Completion::Return(value) = &completion {
                            self.cleanups.push(Cleanup::Completion(value.clone()));
                        }
                        None
                    };
                    self.loops = loops;
                    self.block(&body);
                    self.cleanups.truncate(depth);
                    if self.terminated {
                        return;
                    }
                    if let Some((error, previous)) = error {
                        self.restore_active(&previous, None);
                        self.void_call("relar_error_set", &[&error]);
                        self.release(&error);
                    }
                }
            }
        }
        match completion {
            Completion::Return(value) => {
                self.line(format!(
                    "store {} {}, ptr %function_return",
                    value.ty.llvm(),
                    value.operand
                ));
                self.jump(&self.return_exit.clone());
            }
            Completion::Jump { label, .. } => self.jump(&label),
            Completion::Error => self.jump(&self.error_exit.clone()),
        }
    }

    /// Only errors from the try body enter these handlers. Handler and else
    /// failures first execute finally, then continue to an enclosing handler.
    fn try_statement(
        &mut self,
        body: &[Statement],
        handlers: &[ExceptHandler],
        orelse: &[Statement],
        finalbody: &[Statement],
    ) {
        let depth = self.cleanups.len();
        let end = self.label();
        if !finalbody.is_empty() {
            self.cleanups.push(Cleanup::Finally {
                body: finalbody.to_vec(),
                loops: self.loops.clone(),
            });
        }
        let dispatch = self.label();
        if !handlers.is_empty() {
            self.cleanups.push(Cleanup::Handler(dispatch.clone()));
        }
        self.block(body);
        if !handlers.is_empty() {
            self.cleanups.pop();
        }
        if !self.terminated {
            self.block(orelse);
            if !self.terminated {
                self.route(Completion::Jump {
                    label: end.clone(),
                    depth,
                });
            }
        }
        if !handlers.is_empty() {
            self.start(&dispatch);
            let exception = self.call("relar_error_take", Type::Exception, &[]);
            let pending_temporary = self.temporaries.clone();
            for handler in handlers {
                let matched = if handler.kinds.is_empty() {
                    "true".to_owned()
                } else {
                    let mut matched = "false".to_owned();
                    for kind in &handler.kinds {
                        let byte = self.instruction(format!(
                            "call i8 @relar_exception_matches(ptr {}, i32 {kind})",
                            exception.operand
                        ));
                        let test = self.instruction(format!("icmp ne i8 {byte}, 0"));
                        matched = self.instruction(format!("or i1 {matched}, {test}"));
                    }
                    matched
                };
                let (caught, next) = (self.label(), self.label());
                self.branch(&matched, &caught, &next);
                self.start(&caught);
                let previous = self.call(
                    "relar_exception_swap_active",
                    Type::Exception,
                    &[&exception],
                );
                self.forget(&previous);
                if let Some(name) = handler.name {
                    self.store(name, &exception);
                } else {
                    self.release(&exception);
                }
                self.cleanups.push(Cleanup::Active {
                    previous,
                    name: handler.name,
                });
                self.block(&handler.body);
                if !self.terminated {
                    self.route(Completion::Jump {
                        label: end.clone(),
                        depth,
                    });
                }
                self.cleanups.pop();
                self.start(&next);
                self.temporaries = pending_temporary.clone();
            }
            self.void_call("relar_error_set", &[&exception]);
            self.release(&exception);
            self.route(Completion::Error);
        }
        self.cleanups.truncate(depth);
        self.start(&end);
    }

    /// The caller roots arguments throughout a call. Parameters whose bindings
    /// never change can share that root; rebound parameters need their own.
    fn function(
        &mut self,
        function: &Function,
        symbol: &str,
        entry: bool,
        borrows: &FunctionBorrows,
    ) {
        self.entry_allocas.clear();
        self.locals = function.locals.clone();
        self.borrowed_locals = borrows.locals.clone();
        self.rooted_locals = borrows.rooted.clone();
        self.length_roots = borrows.length_roots.clone();
        self.length_snapshots = vec![None; self.locals.len()];
        self.small_locals = self.ranges.small_locals(function);
        self.expression_rebound = function.expression_rebound();
        let rebound = borrows::written(function);
        self.borrowed_params = vec![false; self.locals.len()];
        for &slot in &function.params {
            self.borrowed_params[slot] = !rebound[slot];
        }
        self.loops.clear();
        self.temporaries.clear();
        self.cleanups.clear();
        self.return_exit = self.label();
        self.error_exit = self.label();
        let params = function
            .params
            .iter()
            .enumerate()
            .map(|(i, &slot)| format!("{} %arg{i}", function.locals[slot].llvm()))
            .collect::<Vec<_>>()
            .join(", ");
        let params = if entry && self.uses_sys_argv {
            "i32 %argc, ptr %argv".to_owned()
        } else {
            params
        };
        let returns = if entry {
            "i32"
        } else {
            function.returns.llvm()
        };
        let linkage = if entry { "" } else { "internal " };
        let attributes = if self.sanitize {
            " sanitize_address"
        } else {
            ""
        };
        writeln!(
            self.code,
            "\ndefine {linkage}{returns} @{symbol}({params}){attributes} {{"
        )
        .unwrap();
        self.start("entry");
        let allocas_at = self.code.len();
        self.line(format!(
            "%function_return = alloca {}",
            function.returns.llvm()
        ));
        self.line(format!(
            "store {} {}, ptr %function_return",
            function.returns.llvm(),
            function.returns.initial()
        ));
        // Runtime out-parameters always write full words, including for bool
        // and None payloads. Results are loaded only after a successful call;
        // each call transfers them before another call can reuse this storage.
        self.line("%mapping_key_word = alloca i64");
        self.line("%mapping_value_word = alloca i64");
        for (slot, &ty) in function.locals.iter().enumerate() {
            self.line(format!("%local{slot} = alloca {}", ty.llvm()));
            // Parameters initialize exactly once below. ASan can relocate their
            // argument stores into its prologue, ahead of a default initializer.
            if !function.params.contains(&slot) {
                self.line(format!(
                    "store {} {}, ptr %local{slot}",
                    ty.llvm(),
                    ty.initial()
                ));
            }
        }
        for (i, &slot) in function.params.iter().enumerate() {
            let value = Value {
                ty: function.locals[slot],
                operand: format!("%arg{i}"),
                small_integer: false,
            };
            if !self.borrowed_params[slot] {
                self.retain(&value);
            }
            // Retains are infallible; every parameter is initialized before
            // body evaluation or any recoverable-error cleanup can begin.
            self.line(format!(
                "store {} {}, ptr %local{slot}",
                value.ty.llvm(),
                value.operand
            ));
        }
        if entry {
            let layout = self.instruction("call { i64, i64 } @relar_seq_layout()");
            self.line(format!(
                "store {{ i64, i64 }} {layout}, ptr @sequence_layout"
            ));
            self.line("call void @relar_init_literals()");
        }
        if entry && self.uses_sys_argv {
            self.line("call void @relar_args_init(i32 %argc, ptr %argv)");
        }
        self.block(&function.body);
        if !self.terminated {
            self.jump(&self.return_exit.clone());
        }
        self.start(&self.return_exit.clone());
        self.cleanup();
        if entry {
            self.cleanup_globals();
            self.line("call void @relar_clear_literals()");
            if self.uses_sys_argv {
                self.line("call void @relar_args_clear()");
            }
            self.void_call("relar_flush_stdout", &[]);
            self.line("ret i32 0");
        } else {
            let value = self.instruction(format!(
                "load {}, ptr %function_return",
                function.returns.llvm()
            ));
            self.line(format!("ret {returns} {value}"));
        }
        self.start(&self.error_exit.clone());
        self.cleanup();
        if entry {
            self.cleanup_globals();
            self.line("call void @relar_clear_literals()");
            if self.uses_sys_argv {
                self.line("call void @relar_args_clear()");
            }
            let status = self.instruction("call i32 @relar_error_unhandled()");
            self.line(format!("ret i32 {status}"));
        } else {
            self.line(format!("ret {returns} {}", function.returns.initial()));
        }
        self.code.push_str("}\n");
        self.code.insert_str(allocas_at, &self.entry_allocas);
    }

    fn block(&mut self, statements: &[Statement]) {
        let incoming = self.temporaries.clone();
        let mut statements = statements.iter().peekable();
        while let Some(statement) = statements.next() {
            if self.terminated {
                break;
            }
            if let Some(next) = statements.peek()
                && self.append_pair(statement, next)
            {
                statements.next();
            } else {
                self.statement(statement);
            }
            debug_assert!(
                self.temporaries
                    .iter()
                    .map(|value| (&value.operand, value.ty))
                    .eq(incoming.iter().map(|value| (&value.operand, value.ty))),
                "statement changed its enclosing expression owners"
            );
        }
    }

    /// Combines two appends whose already-evaluated operands cannot change.
    /// Integer locals are immutable values, and append cannot replace its local
    /// receiver. Allocation exhaustion is fatal for both runtime entrypoints.
    fn append_pair(&mut self, first: &Statement, second: &Statement) -> bool {
        let (
            Statement::Expr(Expr {
                kind: ExprKind::Builtin(Builtin::Append, first),
                ..
            }),
            Statement::Expr(Expr {
                kind: ExprKind::Builtin(Builtin::Append, second),
                ..
            }),
        ) = (first, second)
        else {
            return false;
        };
        let ([receiver, first], [other, second]) = (first.as_slice(), second.as_slice()) else {
            return false;
        };
        let (ExprKind::Local(left), ExprKind::Local(right)) = (&receiver.kind, &other.kind) else {
            return false;
        };
        if left != right
            || receiver.ty != Type::ListInt
            || other.ty != Type::ListInt
            || !matches!(first.kind, ExprKind::Local(_))
            || !matches!(second.kind, ExprKind::Local(_))
            || first.ty != Type::Int
            || second.ty != Type::Int
        {
            return false;
        }
        let receiver = self.operand(receiver);
        let first = self.operand(first);
        let second = self.operand(second);
        self.void_call(
            "relar_list_append_pair",
            &[&receiver.value, &first.value, &second.value],
        );
        for operand in [&receiver, &first, &second] {
            self.release_operand(operand);
        }
        true
    }

    fn statement(&mut self, statement: &Statement) {
        match statement {
            Statement::Assign(slot, expression) => {
                if self.borrowed_locals[*slot] {
                    let value = self.borrow_projection(expression);
                    self.store_borrowed(*slot, &value);
                    return;
                }
                if self.concat_assign(*slot, expression) {
                    return;
                }
                if !self.loops.is_empty()
                    && let ExprKind::Tuple(fields) = &expression.kind
                {
                    self.assign_loop_tuple(*slot, fields);
                    return;
                }
                let previous_small = self.assignment_replaces_small(*slot, expression);
                let value = self.expr(expression);
                self.store_with_previous_small(*slot, &value, previous_small);
            }
            Statement::Move(target, source) => {
                debug_assert_eq!(self.locals[*target], self.locals[*source]);
                if self.borrowed_locals[*source] {
                    let value = self.borrow_slot(*source);
                    if self.borrowed_locals[*target] {
                        self.reset_borrowed(*source);
                        self.store_borrowed(*target, &value);
                    } else {
                        self.retain(&value);
                        self.reset_borrowed(*source);
                        self.store(*target, &value);
                    }
                    return;
                }
                let value = self.take_slot(*source);
                self.store(*target, &value);
            }
            Statement::Clear(slot) => self.clear_slots(&[*slot]),
            Statement::Scope { body, clear } => {
                self.cleanups.push(Cleanup::Slots(clear.clone()));
                self.block(body);
                self.cleanups.pop();
                if !self.terminated {
                    self.clear_slots(clear);
                }
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => self.try_statement(body, handlers, orelse, finalbody),
            Statement::Raise(exception) => {
                if let Some(exception) = exception {
                    let exception = self.operand(exception);
                    self.void_call("relar_exception_raise", &[&exception.value]);
                    self.release_operand(&exception);
                } else {
                    self.void_call("relar_exception_reraise", &[]);
                }
                // Both runtime helpers always set an error; their success edge
                // is unreachable, but uses the same immediate-check protocol.
                self.line("unreachable");
                self.terminated = true;
            }
            Statement::IterNext {
                iterator,
                key,
                value,
                ready,
            } => {
                self.iter_next(*iterator, *key, *value, *ready);
            }
            Statement::SetItem(list, index, expression) => {
                if self.copy_item(list, index, expression) {
                    return;
                }
                // Python evaluates an assignment's RHS before its target expressions.
                let nonnegative = self.nonnegative_index(index);
                let value = self.operand(expression);
                let list = self.operand(list);
                let index = self.operand(index);
                self.set_item(&list.value, &index.value, &value.value, nonnegative);
                self.release_operand(&value);
                self.release_operand(&index);
                self.release_operand(&list);
            }
            Statement::AugmentItem(list, index, op, expression) => {
                // Augmented assignment instead roots the target and old item first.
                let nonnegative = self.nonnegative_index(index);
                let list = self.operand(list);
                let index = self.operand(index);
                let element = self
                    .module
                    .types
                    .element(list.value.ty)
                    .or_else(|| {
                        self.module
                            .types
                            .mapping(list.value.ty)
                            .map(|(_, value)| value)
                    })
                    .unwrap();
                let old = self.get_item(&list.value, &index.value, element, nonnegative);
                let right = self.operand(expression);
                let value = self.binary(*op, &old, &right.value);
                self.set_item(&list.value, &index.value, &value, nonnegative);
                self.release(&value);
                self.release_operand(&right);
                self.release(&old);
                self.release_operand(&index);
                self.release_operand(&list);
            }
            Statement::SetField(receiver, field, expression) => {
                let value = self.operand(expression);
                let receiver = self.operand(receiver);
                self.set_field(&receiver.value, *field, &value.value);
                self.release_operand(&receiver);
                self.release_operand(&value);
            }
            Statement::AugmentField(receiver, field, op, expression) => {
                let receiver = self.operand(receiver);
                let Type::Class(class) = receiver.value.ty else {
                    unreachable!()
                };
                let ty = self.module.class_fields[class][*field];
                // A scalar or a non-reentrant exact length read cannot replace
                // this field. Other right-hand sides must own the original
                // value across arbitrary mutation and exceptions.
                let stable = matches!(expression.kind, ExprKind::Int(_) | ExprKind::Local(_))
                    || matches!(&expression.kind, ExprKind::Builtin(Builtin::Len, values)
                        if matches!(values.as_slice(), [list] if matches!(list.ty, Type::ListInt | Type::List(_))
                            && (matches!(list.kind, ExprKind::Local(_))
                                || borrows::getters::selected(list, &self.module.functions).is_some())));
                let old = if stable && ty == Type::Int {
                    let pointer = self.field_pointer(&receiver.value, *field);
                    Operand {
                        value: self.field_value(&pointer, ty),
                        ownership: Ownership::Borrowed,
                    }
                } else {
                    Operand {
                        value: self.get_field(&receiver.value, *field, ty),
                        ownership: Ownership::Owned,
                    }
                };
                let right = self.operand(expression);
                let value = self.binary(*op, &old.value, &right.value);
                self.forget(&value);
                self.replace_field(&receiver.value, *field, &value);
                self.release_operand(&right);
                self.release_operand(&old);
                self.release_operand(&receiver);
            }
            // Discarded scalar literals have no effects. In particular, a
            // function docstring must not allocate on every call. Literal
            // allocation exhaustion is already fatal rather than catchable.
            Statement::Expr(expression)
                if matches!(
                    expression.kind,
                    ExprKind::Int(_)
                        | ExprKind::Float(_)
                        | ExprKind::Bool(_)
                        | ExprKind::Str(_)
                        | ExprKind::Bytes(_)
                        | ExprKind::None
                ) => {}
            Statement::Expr(expression) => {
                if let ExprKind::Builtin(Builtin::TextWrite, args) = &expression.kind {
                    self.write_discard(args);
                } else {
                    let value = self.expr(expression);
                    self.release(&value);
                }
            }
            Statement::Return(expression) => {
                let value = self.expr(expression);
                self.forget(&value);
                self.route(Completion::Return(value));
            }
            Statement::If(test, yes, no) => {
                let test = self.expr(test);
                let (yes_label, no_label, end) = (self.label(), self.label(), self.label());
                self.branch(&test.operand, &yes_label, &no_label);
                self.start(&yes_label);
                self.block(yes);
                let yes_ends = self.terminated;
                if !yes_ends {
                    self.jump(&end);
                }
                self.start(&no_label);
                self.block(no);
                let no_ends = self.terminated;
                if !no_ends {
                    self.jump(&end);
                }
                self.start(&end);
                if yes_ends && no_ends {
                    self.line("unreachable");
                    self.terminated = true;
                }
            }
            Statement::While(test, body, orelse) => {
                let previous_lengths = self.snapshot_loop_lengths(statement);
                let (condition, body_label, end) = (self.label(), self.label(), self.label());
                let exhausted = if orelse.is_empty() {
                    end.clone()
                } else {
                    self.label()
                };
                self.jump(&condition);
                self.start(&condition);
                let test = self.expr(test);
                self.branch(&test.operand, &body_label, &exhausted);
                self.start(&body_label);
                self.loops.push(LoopTarget {
                    end: end.clone(),
                    exhausted: exhausted.clone(),
                    next: condition.clone(),
                    cleanup_depth: self.cleanups.len(),
                });
                self.block(body);
                self.loops.pop();
                if !self.terminated {
                    self.jump(&condition);
                }
                if !orelse.is_empty() {
                    self.start(&exhausted);
                    self.block(orelse);
                    if !self.terminated {
                        self.jump(&end);
                    }
                }
                self.start(&end);
                self.length_snapshots = previous_lengths;
            }
            Statement::For {
                target,
                cursor,
                stop,
                step,
                args,
                body,
                orelse,
            } => {
                // This proof includes the final increment and the hidden cursor,
                // independently of reassignment of the visible loop target.
                let small_cursor = self.ranges.range_cursor(statement).is_some();
                let mut args = args.iter().map(|arg| self.expr(arg)).collect::<Vec<_>>();
                if args.len() == 1 {
                    args.insert(0, small(0));
                }
                if args.len() == 2 {
                    args.push(small(1));
                }
                for (&slot, value) in [*cursor, *stop, *step].iter().zip(&args) {
                    self.store(slot, value);
                }
                let zero = self.compare(Compare::Equal, &args[2], &small(0));
                let (fail, condition, body_label, increment, end) = (
                    self.label(),
                    self.label(),
                    self.label(),
                    self.label(),
                    self.label(),
                );
                let exhausted = if orelse.is_empty() {
                    end.clone()
                } else {
                    self.label()
                };
                self.branch(&zero.operand, &fail, &condition);
                self.start(&fail);
                self.checked_line("call void @relar_range_step_zero()");
                self.line("unreachable");
                self.start(&condition);
                let mut current = self.borrow_slot(*cursor);
                current.small_integer |= small_cursor;
                let mut stop_value = self.borrow_slot(*stop);
                stop_value.small_integer |= args[1].small_integer;
                let mut step_value = self.borrow_slot(*step);
                step_value.small_integer |= args[2].small_integer;
                let positive = self.compare(Compare::Greater, &step_value, &small(0));
                let below = self.compare(Compare::Less, &current, &stop_value);
                let above = self.compare(Compare::Greater, &current, &stop_value);
                let test = self.instruction(format!(
                    "select i1 {}, i1 {}, i1 {}",
                    positive.operand, below.operand, above.operand
                ));
                self.branch(&test, &body_label, &exhausted);
                self.start(&body_label);
                let current = self.load(*cursor, small_cursor);
                self.store(*target, &current);
                self.loops.push(LoopTarget {
                    end: end.clone(),
                    exhausted: exhausted.clone(),
                    next: increment.clone(),
                    cleanup_depth: self.cleanups.len(),
                });
                self.block(body);
                self.loops.pop();
                if !self.terminated {
                    self.jump(&increment);
                }
                self.start(&increment);
                let mut current = self.borrow_slot(*cursor);
                current.small_integer |= small_cursor;
                let mut step_value = self.borrow_slot(*step);
                step_value.small_integer |= args[2].small_integer;
                let next = if small_cursor {
                    self.small_binary(Binary::Add, &current, &step_value, false, None)
                } else {
                    self.binary(Binary::Add, &current, &step_value)
                };
                self.store(*cursor, &next);
                self.jump(&condition);
                if !orelse.is_empty() {
                    self.start(&exhausted);
                    self.block(orelse);
                    if !self.terminated {
                        self.jump(&end);
                    }
                }
                self.start(&end);
            }
            Statement::Break | Statement::Continue | Statement::Exhaust => {
                let target = self.loops.last().unwrap();
                self.route(Completion::Jump {
                    label: match statement {
                        Statement::Break => target.end.clone(),
                        Statement::Exhaust => target.exhausted.clone(),
                        _ => target.next.clone(),
                    },
                    depth: target.cleanup_depth,
                });
            }
            Statement::Assert(test) => {
                let test = self.expr(test);
                let (success, fail) = (self.label(), self.label());
                self.branch(&test.operand, &success, &fail);
                self.start(&fail);
                self.checked_line("call void @relar_assert_fail()");
                self.line("unreachable");
                self.start(&success);
            }
        }
    }

    fn literal(&mut self, contents: &str, ty: Type) -> Value {
        self.literal_bytes(contents.as_bytes(), ty)
    }

    /// Emits exact source bytes without routing arbitrary byte values through UTF-8.
    fn literal_bytes(&mut self, contents: &[u8], ty: Type) -> Value {
        if ty == Type::Str {
            return self.pooled_literal(contents, Type::Str);
        }
        let id = self.strings.len();
        self.strings.push(contents.to_vec());
        let function = match ty {
            Type::Int => "relar_int_literal",
            Type::Bytes => "relar_bytes_literal",
            _ => unreachable!("literal representation was checked"),
        };
        Value {
            ty,
            operand: self.instruction(format!(
                "call {} @{function}(ptr @str_{id}, i64 {})",
                ty.llvm(),
                contents.len()
            )),
            small_integer: false,
        }
    }

    /// Produces an owned result; its consumer must transfer or release that ownership.
    fn expr(&mut self, expression: &Expr) -> Value {
        let mut value = if self.can_borrow_projection(expression) {
            let value = self.borrow_projection(expression);
            self.retain(&value);
            value
        } else {
            self.expression(expression)
        };
        value.small_integer |= self.ranges.integer(expression).is_some();
        self.owned(value)
    }

    fn expression(&mut self, expression: &Expr) -> Value {
        match &expression.kind {
            ExprKind::Int(value) => {
                if let Ok(value) = value.parse::<i64>()
                    && (-(1_i64 << 62)..(1_i64 << 62)).contains(&value)
                {
                    small(value)
                } else {
                    self.literal(value, Type::Int)
                }
            }
            ExprKind::Float(value) => Value {
                ty: Type::Float,
                operand: format!("0x{:016X}", value.to_bits()),
                small_integer: false,
            },
            ExprKind::Bool(value) => Value {
                ty: Type::Bool,
                operand: u8::from(*value).to_string(),
                small_integer: false,
            },
            ExprKind::Str(value) => Value {
                ty: expression.ty,
                ..self.literal(value, Type::Str)
            },
            ExprKind::Bytes(value) => self.literal_bytes(value, Type::Bytes),
            ExprKind::None => none(),
            ExprKind::Local(slot) => {
                let mut value = self.borrow_slot(*slot);
                value.small_integer |= self.ranges.integer(expression).is_some();
                self.retain(&value);
                value
            }
            ExprKind::Bind(slot, expression) => {
                let value = self.expr(expression);
                self.retain(&value);
                let storage = self.locals[*slot];
                let stored = if storage == value.ty {
                    value.clone()
                } else if value.ty == Type::None {
                    Value {
                        ty: storage,
                        operand: "0".into(),
                        small_integer: false,
                    }
                } else if matches!(
                    self.module.types.payload(storage).unwrap_or(storage),
                    Type::Union(_)
                ) && matches!(
                    self.module.types.payload(value.ty).unwrap_or(value.ty),
                    Type::Str | Type::Bytes
                ) {
                    self.union_inject(value.clone(), storage)
                } else {
                    self.present(value.clone(), storage)
                };
                self.store(*slot, &stored);
                value
            }
            ExprKind::Scope { body, value, clear } => {
                self.cleanups.push(Cleanup::Slots(clear.clone()));
                self.block(body);
                let value = self.expr(value);
                self.cleanups.pop();
                self.clear_slots(clear);
                value
            }
            ExprKind::List(values) => {
                let list = if expression.ty == Type::ListInt {
                    self.call("relar_list_new", expression.ty, &[])
                } else {
                    let element = self.module.types.element(expression.ty).unwrap();
                    let value = Value {
                        ty: expression.ty,
                        operand: self.instruction(format!(
                            "call ptr @relar_seq_new(i32 {})",
                            representation_kind(element, &self.module.types)
                        )),
                        small_integer: false,
                    };
                    self.owned(value)
                };
                for value in values {
                    let value = self.operand(value);
                    self.append(&list, &value.value);
                    self.release_operand(&value);
                }
                list
            }
            ExprKind::Dict {
                key,
                value,
                entries,
            } => {
                let dict = Value {
                    ty: expression.ty,
                    operand: self.instruction(format!(
                        "call ptr @relar_dict_new(i32 {}, i32 {})",
                        representation_kind(*key, &self.module.types),
                        representation_kind(*value, &self.module.types)
                    )),
                    small_integer: false,
                };
                let dict = self.owned(dict);
                for (key, value) in entries {
                    let key = self.operand(key);
                    let value = self.operand(value);
                    self.set_item(&dict, &key.value, &value.value, false);
                    self.release_operand(&value);
                    self.release_operand(&key);
                }
                dict
            }
            ExprKind::Set { element, values } => {
                let set = Value {
                    ty: expression.ty,
                    operand: self.instruction(format!(
                        "call ptr @relar_set_new(i32 {})",
                        representation_kind(*element, &self.module.types)
                    )),
                    small_integer: false,
                };
                let set = self.owned(set);
                for value in values {
                    let value = self.operand(value);
                    self.set_member(&set, &value.value, true);
                    self.release_operand(&value);
                }
                set
            }
            ExprKind::Tuple(values) => {
                let object = self.record(expression.ty);
                for (field, value) in values.iter().enumerate() {
                    let value = self.operand(value);
                    self.set_field(&object, field, &value.value);
                    self.release_operand(&value);
                }
                object
            }
            ExprKind::Present(expression_value) => {
                let value = self.operand(expression_value);
                let value = self.own_operand(value);
                self.present(value, expression.ty)
            }
            ExprKind::Absent(expression_value) => {
                let value = self.expr(expression_value);
                self.release(&value);
                Value {
                    ty: expression.ty,
                    operand: "0".to_owned(),
                    small_integer: false,
                }
            }
            ExprKind::Unwrap(receiver) => {
                let receiver = self.operand(receiver);
                if expression.ty == Type::Float {
                    let value = self.call("relar_float_unbox", Type::Float, &[&receiver.value]);
                    self.release_operand(&receiver);
                    return value;
                }
                let receiver = self.own_operand(receiver);
                self.forget(&receiver);
                let word = if expression.ty == Type::Bool {
                    self.instruction(format!("sub i64 {}, 1", receiver.operand))
                } else {
                    receiver.operand
                };
                self.decode_word(word, expression.ty)
            }
            ExprKind::Binary(Binary::Concat, ..) if expression.ty == Type::Str => {
                self.concat(expression)
            }
            ExprKind::Binary(op, left, right) => {
                let direct = self.small_binary_proof(expression, left, right, *op);
                let left_value = self.operand(left);
                let right_value = self.operand(right);
                let result = if let Some(adjust_floor) = direct {
                    let bounds = self.ranges.integer(left).zip(self.ranges.integer(right));
                    self.small_binary(
                        *op,
                        &left_value.value,
                        &right_value.value,
                        adjust_floor,
                        bounds,
                    )
                } else {
                    self.binary(*op, &left_value.value, &right_value.value)
                };
                self.release_operand(&left_value);
                self.release_operand(&right_value);
                result
            }
            ExprKind::Negate(value) => {
                let direct = self.ranges.integer(expression).is_some()
                    && self.ranges.integer(value).is_some();
                let value = self.operand(value);
                let result = if value.value.ty == Type::Float {
                    Value {
                        ty: Type::Float,
                        operand: self.instruction(format!("fneg double {}", value.value.operand)),
                        small_integer: false,
                    }
                } else if direct {
                    self.small_binary(Binary::Subtract, &small(0), &value.value, false, None)
                } else {
                    self.binary(Binary::Subtract, &small(0), &value.value)
                };
                self.release_operand(&value);
                result
            }
            ExprKind::Not(value) => {
                let value = self.expr(value);
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!("xor i1 {}, true", value.operand)),
                    small_integer: false,
                }
            }
            ExprKind::Boolean(and, values) => self.boolean(*and, values),
            ExprKind::BooleanValue(and, left, right) => {
                self.boolean_value(*and, left, right, expression.ty)
            }
            ExprKind::Conditional(test, yes, no) => self.conditional(test, yes, no),
            ExprKind::Compare(values, ops) => self.comparisons(values, ops),
            ExprKind::Call(id, args) => {
                let values = self.call_operands(args);
                let result = self.user_call(
                    *id,
                    expression.ty,
                    &args
                        .order
                        .iter()
                        .map(|&index| &values[index].value)
                        .collect::<Vec<_>>(),
                    args.exhaustive_enum_match,
                );
                for arg in &values {
                    self.release_operand(arg);
                }
                result
            }
            ExprKind::Callable { function, receiver } => {
                self.callable_value(expression.ty, *function, receiver.as_deref())
            }
            ExprKind::Invoke { callable, args } => {
                self.invoke_callable(callable, args, expression.ty)
            }
            ExprKind::Construct { initializer, args } => {
                // Arguments are evaluated and rooted before __init__ starts.
                let values = self.call_operands(args);
                let object = self.record(expression.ty);
                let mut inputs = vec![&object];
                inputs.extend(args.order.iter().map(|&index| &values[index].value));
                self.user_call(*initializer, Type::None, &inputs, false);
                for arg in &values {
                    self.release_operand(arg);
                }
                object
            }
            ExprKind::Exception { kind, message } => {
                let message = message.as_ref().map(|message| self.operand(message));
                let pointer = message
                    .as_ref()
                    .map_or("null", |message| message.value.operand.as_str());
                let value = Value {
                    ty: Type::Exception,
                    operand: self.instruction(format!(
                        "call ptr @relar_exception_new(i32 {kind}, ptr {pointer})"
                    )),
                    small_integer: false,
                };
                if let Some(message) = &message {
                    self.release_operand(message);
                }
                value
            }
            ExprKind::Field(receiver, field) => {
                let receiver = self.immediate_receiver(receiver);
                let value = self.get_field(&receiver.value, *field, expression.ty);
                self.release_operand(&receiver);
                value
            }
            ExprKind::Builtin(builtin, args) => self.builtin(builtin, args, expression.ty),
        }
    }

    /// Transfers an overwritten string only after every suffix has evaluated.
    /// The local remains rooted on all RHS error paths. Expression-rebound
    /// locals use the ordinary path, since their first value may differ from
    /// the slot's value after a later `:=`. Runtime uniqueness checks preserve
    /// aliases, including suffixes that borrow the overwritten string itself.
    fn concat_assign(&mut self, slot: usize, expression: &Expr) -> bool {
        if self.locals[slot] != Type::Str
            || self.expression_rebound[slot]
            || !matches!(expression.kind, ExprKind::Binary(Binary::Concat, ..))
        {
            return false;
        }
        let expressions = concat_leaves(expression);
        if !matches!(expressions[0].kind, ExprKind::Local(source) if source == slot) {
            return false;
        }
        let disjoint = expressions[1..].iter().all(|expression| {
            matches!(
                expression.kind,
                ExprKind::Str(_) | ExprKind::Builtin(Builtin::StringGet, _)
            )
        });
        let (scratch, count, owners) = self.concat_parts(expressions[1..].to_vec());
        let left = self.take_slot(slot);
        self.forget(&left);
        let instruction = if count == 1 && disjoint {
            // Literals live in static storage; indexed characters were copied
            // into unique scratch (or point to an immutable ASCII literal).
            // Neither can overlap the runtime allocation being consumed.
            let data = self.instruction(format!("load ptr, ptr {scratch}"));
            let length_slot = self.instruction(format!(
                "getelementptr inbounds {{ ptr, i64 }}, ptr {scratch}, i64 0, i32 1"
            ));
            let length = self.instruction(format!("load i64, ptr {length_slot}"));
            format!(
                "call ptr @relar_str_append_utf8_owned(ptr {}, ptr {data}, i64 {length})",
                left.operand
            )
        } else {
            format!(
                "call ptr @relar_str_concat_parts_owned(ptr {}, ptr {scratch}, i64 {count})",
                left.operand
            )
        };
        let value = Value {
            ty: Type::Str,
            operand: self.instruction(instruction),
            small_integer: false,
        };
        let value = self.owned(value);
        for owner in &owners {
            self.release_operand(owner);
        }
        self.store(slot, &value);
        true
    }

    /// Fuses only exact immutable string concatenation. Leaves are evaluated in
    /// source order and computed values stay owned across later mutation/errors;
    /// literal spans need no runtime string owner. Exact str(Int) conversions
    /// still run at their source position, using stack bytes or an owned BigInt
    /// string. Allocation failure is fatal, so removed allocations cannot raise
    /// a caught error.
    /// Unique entry-block scratch avoids loop stack growth and nested overwrite.
    fn concat(&mut self, expression: &Expr) -> Value {
        let expressions = concat_leaves(expression);
        if let [left, right] = expressions.as_slice()
            && !matches!(left.kind, ExprKind::Str(_))
            && !matches!(right.kind, ExprKind::Str(_))
            && !matches!(left.kind, ExprKind::Builtin(Builtin::StringGet, _))
            && !matches!(right.kind, ExprKind::Builtin(Builtin::StringGet, _))
            && integer_string_argument(left).is_none()
            && integer_string_argument(right).is_none()
        {
            let left = self.operand(left);
            let right = self.operand(right);
            let result = self.call("relar_str_concat", Type::Str, &[&left.value, &right.value]);
            self.release_operand(&left);
            self.release_operand(&right);
            return result;
        }

        let (scratch, count, owners) = self.concat_parts(expressions);
        let value = Value {
            ty: Type::Str,
            operand: self.instruction(format!(
                "call ptr @relar_str_concat_parts(ptr {scratch}, i64 {count})"
            )),
            small_integer: false,
        };
        let value = self.owned(value);
        for owner in &owners {
            self.release_operand(owner);
        }
        value
    }

    /// Evaluates and roots ordered leaves before their consumer runs. Returned
    /// owners must survive the consumer's error check; all borrowed byte spans
    /// reside in immutable literals or unique entry-block formatting scratch.
    fn concat_parts(&mut self, expressions: Vec<&Expr>) -> (String, usize, Vec<Operand>) {
        // Adjacent proved ASCII lookups can share one byte span. Capture each
        // character at its original evaluation point before evaluating the next.
        let groups = expressions
            .chunk_by(|left, right| {
                self.ascii_index_literal(left).is_some()
                    && self.ascii_index_literal(right).is_some()
            })
            .collect::<Vec<_>>();
        let count = groups.len();
        let scratch = self.entry_alloca(format!("[{count} x {{ ptr, i64 }}]"));
        let mut owners = Vec::new();
        for (index, group) in groups.into_iter().enumerate() {
            let part = self.instruction(format!(
                "getelementptr inbounds {{ ptr, i64 }}, ptr {scratch}, i64 {index}"
            ));
            if group.len() > 1 {
                let buffer = self.entry_alloca(format!("[{} x i8]", group.len()));
                for (offset, expression) in group.iter().enumerate() {
                    let data = self.literal_index_data(expression).unwrap();
                    let byte = self.instruction(format!("load i8, ptr {data}"));
                    let destination = self.instruction(format!(
                        "getelementptr inbounds i8, ptr {buffer}, i64 {offset}"
                    ));
                    self.line(format!("store i8 {byte}, ptr {destination}"));
                }
                self.store_string_part(&part, &buffer, &group.len().to_string());
                continue;
            }
            let expression = group[0];
            if let ExprKind::Builtin(Builtin::StringGet, args) = &expression.kind {
                if let Some(data) = self.literal_index_data(expression) {
                    self.store_string_part(&part, &data, "1");
                    continue;
                }
                let source = self.operand(&args[0]);
                let at = self.operand(&args[1]);
                let buffer = self.entry_alloca("[4 x i8]");
                let status = self.instruction(format!(
                    "call i8 @relar_str_get_part(ptr {}, i64 {}, ptr {buffer}, ptr {part})",
                    source.value.operand, at.value.operand
                ));
                let failed = self.instruction(format!("icmp eq i8 {status}, 0"));
                self.check_failed(&failed);
                self.release_operand(&source);
                self.release_operand(&at);
                continue;
            }
            if let Some(integer) = integer_string_argument(expression) {
                let integer = self.operand(integer);
                let buffer = self.entry_alloca("[20 x i8]");
                let value = Value {
                    ty: Type::Str,
                    operand: self.instruction(format!(
                        "call ptr @relar_int_str_part(i64 {}, ptr {buffer}, ptr {part})",
                        integer.value.operand
                    )),
                    small_integer: false,
                };
                // Null denotes inline bytes, not failure; release is null-safe.
                let value = self.owned(value);
                owners.push(Operand {
                    value,
                    ownership: Ownership::Owned,
                });
                self.release_operand(&integer);
                continue;
            }
            let (data, length) = if let ExprKind::Str(value) = &expression.kind {
                let id = self.strings.len();
                self.strings.push(value.as_bytes().to_vec());
                (format!("@str_{id}"), value.len().to_string())
            } else {
                let operand = self.operand(expression);
                let data = operand.value.operand.clone();
                owners.push(operand);
                (data, "-1".to_owned())
            };
            self.store_string_part(&part, &data, &length);
        }
        (scratch, count, owners)
    }

    /// A bounded index into an immutable ASCII literal names one UTF-8 byte.
    /// Fixed module bindings qualify only when initialized by that literal;
    /// checking has already rejected reads before initialization and rebinding.
    /// Unknown or invalid indices retain the ordinary checked Unicode path.
    fn ascii_index_literal<'a>(&'a self, expression: &'a Expr) -> Option<&'a str> {
        let ExprKind::Builtin(Builtin::StringGet, args) = &expression.kind else {
            return None;
        };
        let range = self.ranges.integer(&args[1])?;
        let text = match &args[0].kind {
            ExprKind::Str(text) => Some(text.as_str()),
            ExprKind::Builtin(Builtin::GlobalGet(slot), _) => {
                self.literal_globals[*slot].as_deref()
            }
            _ => None,
        };
        text.filter(|text| text.is_ascii() && range.min >= 0 && (range.max as usize) < text.len())
    }

    /// Evaluates the index and returns a static one-byte UTF-8 span only when
    /// literal identity and bounds were proved. Index effects retain their order.
    fn literal_index_data(&mut self, expression: &Expr) -> Option<String> {
        let bytes = self.ascii_index_literal(expression)?.as_bytes().to_vec();
        let ExprKind::Builtin(Builtin::StringGet, args) = &expression.kind else {
            unreachable!();
        };
        let index = self.operand(&args[1]);
        let index_value = self.instruction(format!("ashr i64 {}, 1", index.value.operand));
        let id = self.strings.len();
        self.strings.push(bytes);
        let data = self.instruction(format!(
            "getelementptr inbounds i8, ptr @str_{id}, i64 {index_value}"
        ));
        self.release_operand(&index);
        Some(data)
    }

    /// Writes the shared raw-span/owned-handle descriptor consumed by builders.
    fn store_string_part(&mut self, part: &str, data: &str, length: &str) {
        let data_slot = self.instruction(format!(
            "getelementptr inbounds {{ ptr, i64 }}, ptr {part}, i64 0, i32 0"
        ));
        self.line(format!("store ptr {data}, ptr {data_slot}"));
        let length_slot = self.instruction(format!(
            "getelementptr inbounds {{ ptr, i64 }}, ptr {part}, i64 0, i32 1"
        ));
        self.line(format!("store i64 {length}, ptr {length_slot}"));
    }

    /// A discarded write result needs no Unicode character count. The runtime
    /// still performs the same state checks and one write, with pending clear
    /// on entry. Exact concat parts are fully assembled before file-state checks;
    /// receiver and fragment roots survive until the status has been checked.
    fn write_discard(&mut self, args: &[Expr]) {
        let receiver = self.operand(&args[0]);
        let (status, owners) = if matches!(args[1].kind, ExprKind::Binary(Binary::Concat, ..)) {
            let (scratch, count, owners) = self.concat_parts(concat_leaves(&args[1]));
            let status = self.instruction(format!(
                "call i8 @relar_text_write_parts(ptr {}, ptr {scratch}, i64 {count})",
                receiver.value.operand
            ));
            (status, owners)
        } else {
            let text = self.operand(&args[1]);
            let status = self.instruction(format!(
                "call i8 @relar_text_write_discard(ptr {}, ptr {})",
                receiver.value.operand, text.value.operand
            ));
            (status, vec![text])
        };
        let failed = self.instruction(format!("icmp eq i8 {status}, 0"));
        self.check_failed(&failed);
        for owner in &owners {
            self.release_operand(owner);
        }
        self.release_operand(&receiver);
    }

    fn binary(&mut self, op: Binary, left: &Value, right: &Value) -> Value {
        if left.ty == Type::Float || right.ty == Type::Float {
            let left = if left.ty == Type::Int {
                self.call("relar_int_float", Type::Float, &[left])
            } else {
                left.clone()
            };
            let right = if right.ty == Type::Int {
                self.call("relar_int_float", Type::Float, &[right])
            } else {
                right.clone()
            };
            let operand = match op {
                Binary::Add | Binary::Subtract | Binary::Multiply => {
                    let instruction = match op {
                        Binary::Add => "fadd",
                        Binary::Subtract => "fsub",
                        Binary::Multiply => "fmul",
                        _ => unreachable!(),
                    };
                    self.instruction(format!(
                        "{instruction} double {}, {}",
                        left.operand, right.operand
                    ))
                }
                Binary::Divide | Binary::FloorDivide | Binary::Modulo => {
                    let operation = match op {
                        Binary::Divide => 0,
                        Binary::FloorDivide => 1,
                        Binary::Modulo => 2,
                        _ => unreachable!(),
                    };
                    self.checked_instruction(format!(
                        "call double @relar_float_divide(double {}, double {}, i32 {operation})",
                        left.operand, right.operand
                    ))
                }
                Binary::Repeat
                | Binary::Concat
                | Binary::BitAnd
                | Binary::BitOr
                | Binary::BitXor
                | Binary::LeftShift
                | Binary::RightShift => {
                    unreachable!("float arithmetic was checked")
                }
            };
            return Value {
                ty: Type::Float,
                operand,
                small_integer: false,
            };
        }
        match op {
            Binary::Repeat => {
                let (list, count) =
                    if matches!(left.ty, Type::ListInt | Type::List(_) | Type::Bytes) {
                        (left, right)
                    } else {
                        (right, left)
                    };
                let name = if list.ty == Type::Bytes {
                    "relar_bytes_repeat"
                } else if list.ty == Type::ListInt {
                    "relar_list_repeat"
                } else {
                    "relar_seq_repeat"
                };
                self.call(name, list.ty, &[list, count])
            }
            Binary::Concat => self.call(
                if left.ty == Type::Bytes {
                    "relar_bytes_concat"
                } else {
                    "relar_str_concat"
                },
                left.ty,
                &[left, right],
            ),
            Binary::Add => self.call("int_add", Type::Int, &[left, right]),
            Binary::Subtract => self.call("int_sub", Type::Int, &[left, right]),
            Binary::Multiply => self.call("int_mul", Type::Int, &[left, right]),
            Binary::Divide => self.call("relar_int_true_divide", Type::Float, &[left, right]),
            Binary::FloorDivide => self.call("int_floordiv", Type::Int, &[left, right]),
            Binary::Modulo => self.call("int_mod", Type::Int, &[left, right]),
            Binary::BitAnd => self.call("int_and", Type::Int, &[left, right]),
            Binary::BitOr => self.call("int_or", Type::Int, &[left, right]),
            Binary::BitXor => self.call("int_xor", Type::Int, &[left, right]),
            Binary::LeftShift => self.call("int_lshift", Type::Int, &[left, right]),
            Binary::RightShift => self.call("int_rshift", Type::Int, &[left, right]),
        }
    }

    /// Requires immediate operands and result, plus a nonzero divisor for
    /// division. The optional result says whether Python's floor correction
    /// can still be necessary. Missing proofs retain checked runtime arithmetic.
    fn small_binary_proof(
        &self,
        expression: &Expr,
        left: &Expr,
        right: &Expr,
        op: Binary,
    ) -> Option<bool> {
        self.ranges.integer(expression)?;
        let left = self.ranges.integer(left)?;
        let right = self.ranges.integer(right)?;
        match op {
            Binary::Add
            | Binary::Subtract
            | Binary::Multiply
            | Binary::BitAnd
            | Binary::BitOr
            | Binary::BitXor => Some(false),
            Binary::LeftShift | Binary::RightShift => {
                (right.min >= 0 && right.max < 63).then_some(false)
            }
            Binary::FloorDivide | Binary::Modulo => {
                if right.min <= 0 && right.max >= 0 {
                    return None;
                }
                let same_sign =
                    (left.min >= 0 && right.min > 0) || (left.max <= 0 && right.max < 0);
                Some(!same_sign)
            }
            Binary::Divide | Binary::Repeat | Binary::Concat => None,
        }
    }

    /// Emits arithmetic only after range analysis proves every value fits the
    /// immediate representation and division cannot encounter zero. The result
    /// stays tagged, preserving the ordinary calling and ownership convention.
    /// Both the result and twice the result fit signed i64, so `nsw` is valid
    /// for the arithmetic and encoding shift. This lets LLVM remove redundant
    /// decode/encode round trips without imposing preconditions on Python code.
    fn small_binary(
        &mut self,
        op: Binary,
        left: &Value,
        right: &Value,
        adjust_floor: bool,
        bounds: Option<(IntRange, IntRange)>,
    ) -> Value {
        debug_assert!(left.small_integer && right.small_integer);
        let left = self.instruction(format!("ashr i64 {}, 1", left.operand));
        let right = self.instruction(format!("ashr i64 {}, 1", right.operand));
        let right_constant = bounds
            .map(|(_, right)| right)
            .filter(|right| right.min == right.max)
            .map(|right| right.min);
        let result = match op {
            Binary::Add | Binary::Subtract | Binary::Multiply => {
                let instruction = match op {
                    Binary::Add => "add",
                    Binary::Subtract => "sub",
                    Binary::Multiply => "mul",
                    _ => unreachable!(),
                };
                self.instruction(format!("{instruction} nsw i64 {left}, {right}"))
            }
            Binary::FloorDivide | Binary::Modulo
                if right_constant
                    .is_some_and(|value| value > 0 && (value as u64).is_power_of_two()) =>
            {
                let divisor = right_constant.unwrap() as u64;
                // Arithmetic right shift already rounds toward negative infinity.
                // The corresponding nonnegative remainder is the low-bit mask.
                if matches!(op, Binary::FloorDivide) {
                    self.instruction(format!("ashr i64 {left}, {}", divisor.trailing_zeros()))
                } else {
                    self.instruction(format!("and i64 {left}, {}", divisor - 1))
                }
            }
            Binary::FloorDivide | Binary::Modulo
                if bounds.is_some_and(|(left, right)| {
                    left.min >= 0 && right.min > 0 && left.max < 2 * right.min
                }) =>
            {
                // A nonnegative numerator below twice a positive divisor has
                // quotient zero or one. Both operands were still evaluated,
                // and their small-integer bounds make the subtraction exact.
                let at_least = self.instruction(format!("icmp uge i64 {left}, {right}"));
                if matches!(op, Binary::FloorDivide) {
                    self.instruction(format!("zext i1 {at_least} to i64"))
                } else {
                    let subtract =
                        self.instruction(format!("select i1 {at_least}, i64 {right}, i64 0"));
                    self.instruction(format!("sub nsw i64 {left}, {subtract}"))
                }
            }
            Binary::FloorDivide | Binary::Modulo => {
                let instruction = if matches!(op, Binary::FloorDivide) {
                    "sdiv"
                } else {
                    "srem"
                };
                let result = self.instruction(format!("{instruction} i64 {left}, {right}"));
                if adjust_floor {
                    let remainder = if matches!(op, Binary::Modulo) {
                        result.clone()
                    } else {
                        self.instruction(format!("srem i64 {left}, {right}"))
                    };
                    let nonzero = self.instruction(format!("icmp ne i64 {remainder}, 0"));
                    let signs = self.instruction(format!("xor i64 {remainder}, {right}"));
                    let opposite = self.instruction(format!("icmp slt i64 {signs}, 0"));
                    let adjust = self.instruction(format!("and i1 {nonzero}, {opposite}"));
                    if matches!(op, Binary::FloorDivide) {
                        let adjust = self.instruction(format!("zext i1 {adjust} to i64"));
                        self.instruction(format!("sub nsw i64 {result}, {adjust}"))
                    } else {
                        let adjust =
                            self.instruction(format!("select i1 {adjust}, i64 {right}, i64 0"));
                        self.instruction(format!("add nsw i64 {result}, {adjust}"))
                    }
                } else {
                    result
                }
            }
            Binary::BitAnd
            | Binary::BitOr
            | Binary::BitXor
            | Binary::LeftShift
            | Binary::RightShift => {
                let instruction = match op {
                    Binary::BitAnd => "and",
                    Binary::BitOr => "or",
                    Binary::BitXor => "xor",
                    Binary::LeftShift => "shl nsw",
                    Binary::RightShift => "ashr",
                    _ => unreachable!(),
                };
                self.instruction(format!("{instruction} i64 {left}, {right}"))
            }
            Binary::Divide | Binary::Repeat | Binary::Concat => {
                unreachable!("only proved integer arithmetic")
            }
        };
        let shifted = self.instruction(format!("shl nsw i64 {result}, 1"));
        Value {
            ty: Type::Int,
            operand: self.instruction(format!("or i64 {shifted}, 1")),
            small_integer: true,
        }
    }

    fn compare(&mut self, op: Compare, left: &Value, right: &Value) -> Value {
        let predicate = match op {
            Compare::Equal => "eq",
            Compare::NotEqual => "ne",
            Compare::Less => "slt",
            Compare::LessEqual => "sle",
            Compare::Greater => "sgt",
            Compare::GreaterEqual => "sge",
        };
        if matches!(
            (left.ty, right.ty),
            (Type::Int, Type::Float) | (Type::Float, Type::Int)
        ) {
            let (integer, float, predicate) = if left.ty == Type::Int {
                (left, right, predicate)
            } else {
                let predicate = match op {
                    Compare::Equal => "eq",
                    Compare::NotEqual => "ne",
                    Compare::Less => "sgt",
                    Compare::LessEqual => "sge",
                    Compare::Greater => "slt",
                    Compare::GreaterEqual => "sle",
                };
                (right, left, predicate)
            };
            let order = self.instruction(format!(
                "call i32 @relar_int_float_compare(i64 {}, double {})",
                integer.operand, float.operand
            ));
            let mut operand = self.instruction(format!("icmp {predicate} i32 {order}, 0"));
            if !matches!(op, Compare::Equal | Compare::NotEqual) {
                let ordered = self.instruction(format!("icmp ne i32 {order}, 2"));
                operand = self.instruction(format!("and i1 {ordered}, {operand}"));
            }
            return Value {
                ty: Type::Bool,
                operand,
                small_integer: false,
            };
        }
        let operand = match left.ty {
            Type::Int if left.small_integer && right.small_integer => self.instruction(format!(
                "icmp {predicate} i64 {}, {}",
                left.operand, right.operand
            )),
            Type::Int => {
                let order = self.instruction(format!(
                    "call i32 @compare_int(i64 {}, i64 {})",
                    left.operand, right.operand
                ));
                self.instruction(format!("icmp {predicate} i32 {order}, 0"))
            }
            Type::Float => {
                let predicate = match op {
                    Compare::Equal => "oeq",
                    Compare::NotEqual => "une",
                    Compare::Less => "olt",
                    Compare::LessEqual => "ole",
                    Compare::Greater => "ogt",
                    Compare::GreaterEqual => "oge",
                };
                self.instruction(format!(
                    "fcmp {predicate} double {}, {}",
                    left.operand, right.operand
                ))
            }
            Type::StrEnum(_) => self.instruction(format!(
                "icmp {predicate} i64 {}, {}",
                left.operand, right.operand
            )),
            Type::Str | Type::StrLiteral(_) => {
                let equal = self.instruction(format!(
                    "call i8 @relar_str_equal(ptr {}, ptr {})",
                    left.operand, right.operand
                ));
                let predicate = if matches!(op, Compare::Equal) {
                    "ne"
                } else {
                    "eq"
                };
                self.instruction(format!("icmp {predicate} i8 {equal}, 0"))
            }
            Type::Path => {
                let equal = self.instruction(format!(
                    "call i8 @relar_path_equal(ptr {}, ptr {})",
                    left.operand, right.operand
                ));
                let predicate = if matches!(op, Compare::Equal) {
                    "ne"
                } else {
                    "eq"
                };
                self.instruction(format!("icmp {predicate} i8 {equal}, 0"))
            }
            Type::Bytes => {
                let order = self.instruction(format!(
                    "call i32 @relar_bytes_compare(ptr {}, ptr {})",
                    left.operand, right.operand
                ));
                self.instruction(format!("icmp {predicate} i32 {order}, 0"))
            }
            Type::Class(class)
                if self
                    .module
                    .class_metadata
                    .get(class)
                    .is_some_and(|metadata| metadata.dataclass_eq) =>
            {
                let equal = self.instruction(format!(
                    "call i8 @relar_record_equal(ptr {}, ptr {})",
                    left.operand, right.operand
                ));
                let predicate = if matches!(op, Compare::Equal) {
                    "ne"
                } else {
                    "eq"
                };
                self.instruction(format!("icmp {predicate} i8 {equal}, 0"))
            }
            Type::Tuple(_) => {
                if matches!(op, Compare::Equal | Compare::NotEqual) {
                    let equal = self.instruction(format!(
                        "call i8 @relar_record_equal(ptr {}, ptr {})",
                        left.operand, right.operand
                    ));
                    let predicate = if matches!(op, Compare::Equal) {
                        "ne"
                    } else {
                        "eq"
                    };
                    self.instruction(format!("icmp {predicate} i8 {equal}, 0"))
                } else {
                    let order = self.instruction(format!(
                        "call i32 @relar_tuple_compare(ptr {}, ptr {})",
                        left.operand, right.operand
                    ));
                    self.instruction(format!("icmp {predicate} i32 {order}, 0"))
                }
            }
            Type::Bool
            | Type::None
            | Type::Class(_)
            | Type::Exception
            | Type::TimeoutExpired
            | Type::TextIO
            | Type::CompletedProcess
            | Type::BinaryIO => self.instruction(format!(
                "icmp {predicate} {} {}, {}",
                left.ty.llvm(),
                left.operand,
                right.operand
            )),
            Type::ListInt
            | Type::List(_)
            | Type::Dict(_)
            | Type::Set(_)
            | Type::Iterator
            | Type::Optional(_)
            | Type::Callable(_)
            | Type::RegexPattern
            | Type::RegexMatch
            | Type::Union(_) => {
                unreachable!("container comparison is rejected by the frontend")
            }
        };
        Value {
            ty: Type::Bool,
            operand,
            small_integer: false,
        }
    }

    /// Keeps a chain's middle operand rooted until its final comparison completes.
    fn comparisons(&mut self, values: &[Expr], ops: &[Compare]) -> Value {
        if let ([left, right], [op]) = (values, ops)
            && let Some(result) = self.integer_region_comparison(left, right, *op)
        {
            return result;
        }
        if let ([left, right], [op]) = (values, ops)
            && read_only_integer(left)
            && read_only_integer(right)
        {
            let left = self.comparison_operand(left);
            let right = self.comparison_operand(right);
            let result = self.compare(*op, &left.value, &right.value);
            self.release_operand(&left);
            self.release_operand(&right);
            return result;
        }
        let end = self.label();
        let mut incoming = Vec::new();
        let mut left = self.operand(&values[0]);
        for (i, &op) in ops.iter().enumerate() {
            let right = self.operand(&values[i + 1]);
            let test = self.compare(op, &left.value, &right.value);
            self.release_operand(&left);
            if i + 1 == ops.len() {
                self.release_operand(&right);
                incoming.push(format!("[ {}, %{} ]", test.operand, self.current));
                self.jump(&end);
            } else {
                let (next, fail) = (self.label(), self.label());
                self.branch(&test.operand, &next, &fail);
                let temporaries = self.temporaries.clone();
                self.start(&fail);
                self.release_operand(&right);
                incoming.push(format!("[ false, %{} ]", self.current));
                self.jump(&end);
                self.start(&next);
                self.temporaries = temporaries;
            }
            left = right;
        }
        self.start(&end);
        Value {
            ty: Type::Bool,
            operand: self.instruction(format!("phi i1 {}", incoming.join(", "))),
            small_integer: false,
        }
    }

    /// Requires both complete operands of a two-value integer comparison to
    /// pass `read_only_integer`. A direct local roots the list, and neither
    /// operand can remove or replace its elements before the comparison uses
    /// this unretained word. Only the immediate get is borrowed; arithmetic and
    /// nested reads keep their ordinary owned-result lowering.
    fn comparison_operand(&mut self, expression: &Expr) -> Operand {
        debug_assert!(read_only_integer(expression));
        if let ExprKind::Builtin(Builtin::ListGet, args) = &expression.kind
            && let [receiver, index] = args.as_slice()
            && receiver.ty == Type::ListInt
            && matches!(receiver.kind, ExprKind::Local(_))
            && self.nonnegative_index(index)
        {
            let list = self.operand(receiver);
            let index = self.operand(index);
            let at = self.index_word(&index.value, true);
            let word = self.checked_int_list_word(format!(
                "call i64 @relar_list_borrow_index(ptr {}, i64 {at})",
                list.value.operand
            ));
            self.release_operand(&index);
            self.release_operand(&list);
            let value = self.unpack_word(word, Type::Int);
            self.forget(&value);
            return Operand {
                value,
                ownership: Ownership::Borrowed,
            };
        }
        self.operand(expression)
    }

    /// Each predecessor transfers one result owner into the phi. Removing that
    /// branch's temporary before emitting the other branch keeps its error
    /// cleanup from referring to an SSA value that does not dominate it.
    fn conditional(&mut self, test: &Expr, yes: &Expr, no: &Expr) -> Value {
        if let Some(value) = conditionals::absolute_operand(test, yes, no) {
            let value = self.operand(value);
            let result = self.call("int_abs", Type::Int, &[&value.value]);
            self.release_operand(&value);
            return result;
        }
        let test = self.expr(test);
        let (yes_label, no_label, end) = (self.label(), self.label(), self.label());
        self.branch(&test.operand, &yes_label, &no_label);
        self.start(&yes_label);
        let yes = self.expr(yes);
        self.forget(&yes);
        let yes_end = self.current.clone();
        self.jump(&end);
        self.start(&no_label);
        let no = self.expr(no);
        self.forget(&no);
        let no_end = self.current.clone();
        self.jump(&end);
        self.start(&end);
        Value {
            ty: yes.ty,
            operand: self.instruction(format!(
                "phi {} [ {}, %{yes_end} ], [ {}, %{no_end} ]",
                yes.ty.llvm(),
                yes.operand,
                no.operand
            )),
            small_integer: yes.small_integer && no.small_integer,
        }
    }

    fn boolean(&mut self, and: bool, values: &[Expr]) -> Value {
        let end = self.label();
        let mut incoming = Vec::new();
        for (i, expression) in values.iter().enumerate() {
            let value = self.expr(expression);
            incoming.push(format!("[ {}, %{} ]", value.operand, self.current));
            if i + 1 == values.len() {
                self.jump(&end);
            } else {
                let next = self.label();
                if and {
                    self.branch(&value.operand, &next, &end);
                } else {
                    self.branch(&value.operand, &end, &next);
                }
                self.start(&next);
            }
        }
        self.start(&end);
        Value {
            ty: Type::Bool,
            operand: self.instruction(format!("phi i1 {}", incoming.join(", "))),
            small_integer: false,
        }
    }

    fn builtin(&mut self, builtin: &Builtin, expressions: &[Expr], returns: Type) -> Value {
        if matches!(builtin, Builtin::LiteralStringView) {
            let value = self.expr(&expressions[0]);
            return Value {
                ty: returns,
                ..value
            };
        }
        if let Some(value) = self.rsplit_last(builtin, expressions) {
            return value;
        }
        // Truth consumes immediately and cannot rebind the slot. Transfer the
        // RHS directly to its binding and inspect the newly established owner.
        if matches!(builtin, Builtin::Bool)
            && let [
                Expr {
                    kind: ExprKind::Bind(slot, rhs),
                    ..
                },
            ] = expressions
            && self.locals[*slot] == rhs.ty
        {
            let value = self.expr(rhs);
            self.store(*slot, &value);
            return self.truth(&value);
        }
        let args = expressions
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                if index == 0
                    && matches!(
                        builtin,
                        Builtin::Len | Builtin::Bool | Builtin::RegexCapture { .. }
                    )
                {
                    self.immediate_receiver(arg)
                } else {
                    self.operand(arg)
                }
            })
            .collect::<Vec<_>>();
        let result = match builtin {
            Builtin::StrEnumMember { class, member } => Value {
                ty: returns,
                operand: self.enum_tag(*class, *member).to_string(),
                small_integer: false,
            },
            Builtin::StrEnumName | Builtin::StrEnumValue => {
                self.enum_text(&args[0].value, matches!(builtin, Builtin::StrEnumName))
            }
            Builtin::StrEnumIdentity => Value {
                ty: Type::Bool,
                operand: self.instruction(format!(
                    "icmp eq i64 {}, {}",
                    args[0].value.operand, args[1].value.operand
                )),
                small_integer: false,
            },
            Builtin::LiteralStringView => {
                unreachable!("string views transfer the operand owner directly")
            }
            Builtin::StringListContains => {
                let byte = self.instruction(format!(
                    "call i8 @relar_str_list_contains(ptr {}, ptr {})",
                    args[0].value.operand, args[1].value.operand
                ));
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!("icmp ne i8 {byte}, 0")),
                    small_integer: false,
                }
            }
            Builtin::Text(operation) => self.text_builtin(*operation, &args, returns),
            Builtin::RegexCompile(pattern) => self.pooled_literal(pattern.as_bytes(), returns),
            Builtin::RegexSearch => {
                let pointer = self.instruction(format!(
                    "call ptr @relar_regex_search(ptr {}, ptr {})",
                    args[0].value.operand, args[1].value.operand
                ));
                Value {
                    ty: returns,
                    operand: self.instruction(format!("ptrtoint ptr {pointer} to i64")),
                    small_integer: false,
                }
            }
            Builtin::RegexCapture { operation, known } => {
                let selector = &args[1].value;
                let group = self.word(selector);
                let named = u8::from(selector.ty == Type::Str);
                let receiver = &args[0].value.operand;
                let call = match operation {
                    RegexCapture::Absent => format!(
                        "call i8 @relar_regex_present(ptr {receiver}, i64 {group}, i8 {named})"
                    ),
                    RegexCapture::Start | RegexCapture::End => format!(
                        "call i64 @relar_regex_bound(ptr {receiver}, i64 {group}, i8 {named}, i8 {})",
                        u8::from(*operation == RegexCapture::End)
                    ),
                    RegexCapture::Group => format!(
                        "call ptr @relar_regex_group(ptr {receiver}, i64 {group}, i8 {named})"
                    ),
                };
                let operand = if *known {
                    self.instruction(call)
                } else {
                    self.checked_instruction(call)
                };
                let operand = if *operation == RegexCapture::Absent {
                    self.instruction(format!("icmp eq i8 {operand}, 0"))
                } else if matches!(returns, Type::Optional(_)) {
                    self.instruction(format!("ptrtoint ptr {operand} to i64"))
                } else {
                    operand
                };
                Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                }
            }
            Builtin::Path(operation) => self.path_builtin(*operation, &args, returns),
            Builtin::Process(operation) => self.process_builtin(*operation, &args, returns),
            Builtin::JsonDecode(schema) | Builtin::TomlDecode(schema) => {
                let format = if matches!(builtin, Builtin::TomlDecode(_)) {
                    "toml"
                } else {
                    "json"
                };
                Value {
                    ty: returns,
                    operand: self.checked_instruction(format!(
                        "call ptr @relar_{format}_decode(ptr {}, ptr @{})",
                        args[0].value.operand,
                        json::schema_name(*schema),
                    )),
                    small_integer: false,
                }
            }
            Builtin::JsonEncode(schema) => {
                let word = self.word(&args[0].value);
                let ascii = self.instruction(format!("zext i1 {} to i8", args[2].value.operand));
                Value {
                    ty: returns,
                    operand: self.checked_instruction(format!(
                        "call ptr @relar_json_encode(i64 {word}, ptr @{}, i64 {}, i8 {ascii})",
                        json::schema_name(*schema),
                        args[1].value.operand,
                    )),
                    small_integer: false,
                }
            }
            Builtin::SortByKeys
            | Builtin::StringJoin
            | Builtin::StringReplace
            | Builtin::FloatFormat { .. }
            | Builtin::Abs => self.stdlib_builtin(builtin, &args),
            Builtin::TupleCacheNew => return self.record(returns),
            Builtin::TupleCacheResult => {
                self.call("relar_record_cached_result", returns, &[&args[0].value])
            }
            Builtin::GlobalGet(slot) => {
                let value = self.borrow_global(*slot, returns);
                self.retain(&value);
                value
            }
            Builtin::GlobalSet(slot) => {
                let value = &args[0].value;
                self.retain(value);
                self.line(format!(
                    "store {} {}, ptr @module_value_{slot}",
                    value.ty.llvm(),
                    value.operand
                ));
                none()
            }
            Builtin::UnionInject => {
                let value = self.own_operand(args.into_iter().next().unwrap());
                return self.union_inject(value, returns);
            }
            Builtin::UnionProject => {
                let value = self.own_operand(args.into_iter().next().unwrap());
                return self.union_project(value, returns);
            }
            Builtin::IsInstance(classes) => Value {
                ty: Type::Bool,
                operand: self.instance_test(&args[0].value, classes),
                small_integer: false,
            },
            Builtin::Bool => self.truth(&args[0].value),
            Builtin::Input => self.call("relar_input", Type::Str, &[]),
            Builtin::SysArgv => self.call("relar_sys_argv", returns, &[]),
            Builtin::TextOpen(mode) => Value {
                ty: Type::TextIO,
                operand: self.checked_instruction(format!(
                    "call ptr @relar_text_open(ptr {}, i32 {mode})",
                    args[0].value.operand
                )),
                small_integer: false,
            },
            Builtin::BinaryOpen(mode) => Value {
                ty: Type::BinaryIO,
                operand: self.checked_instruction(format!(
                    "call ptr @relar_binary_open(ptr {}, i32 {mode})",
                    args[0].value.operand
                )),
                small_integer: false,
            },
            Builtin::BinaryRead => self.call("relar_binary_read", Type::Bytes, &[&args[0].value]),
            Builtin::GzipOpen => Value {
                ty: returns,
                operand: self.checked_instruction(format!(
                    "call ptr @relar_gzip_open(ptr {}, i32 {})",
                    args[0].value.operand,
                    u32::from(returns == Type::BinaryIO)
                )),
                small_integer: false,
            },
            Builtin::BinaryReadSize => self.call(
                "relar_binary_read_size",
                Type::Bytes,
                &[&args[0].value, &args[1].value],
            ),
            Builtin::FileReadline => {
                let function = if returns == Type::Bytes {
                    "relar_binary_readline"
                } else {
                    "relar_text_readline"
                };
                self.call(function, returns, &[&args[0].value])
            }
            Builtin::BinaryWrite => self.call(
                "relar_binary_write",
                Type::Int,
                &[&args[0].value, &args[1].value],
            ),
            Builtin::Bytes => match args[0].value.ty {
                Type::Bytes => return self.own_operand(args.into_iter().next().unwrap()),
                Type::Int => self.call("relar_bytes_count", Type::Bytes, &[&args[0].value]),
                Type::ListInt => self.call("relar_bytes_list", Type::Bytes, &[&args[0].value]),
                _ => unreachable!("bytes conversion was checked"),
            },
            Builtin::Encode => self.call("relar_str_encode", Type::Bytes, &[&args[0].value]),
            Builtin::Decode => self.call("relar_bytes_decode", Type::Str, &[&args[0].value]),
            Builtin::BytesValidateUtf8 => {
                self.call("relar_bytes_validate_utf8", Type::Bytes, &[&args[0].value])
            }
            Builtin::TextRead => self.call("relar_text_read", Type::Str, &[&args[0].value]),
            Builtin::TextWrite => self.call(
                "relar_text_write",
                Type::Int,
                &[&args[0].value, &args[1].value],
            ),
            Builtin::TextEnter | Builtin::TextFlush | Builtin::TextClose => {
                let name = match builtin {
                    Builtin::TextEnter => "relar_text_enter",
                    Builtin::TextFlush => "relar_text_flush",
                    Builtin::TextClose => "relar_text_close",
                    _ => unreachable!(),
                };
                self.checked_instruction(format!("call i8 @{name}(ptr {})", args[0].value.operand));
                none()
            }
            Builtin::TextClosed => {
                let byte = self.instruction(format!(
                    "call i8 @relar_text_closed(ptr {})",
                    args[0].value.operand
                ));
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!("icmp ne i8 {byte}, 0")),
                    small_integer: false,
                }
            }
            Builtin::ExceptionMessage => {
                self.call("relar_exception_message", Type::Str, &[&args[0].value])
            }
            Builtin::ExceptionWithCause => {
                let cause =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[1].value.operand));
                Value {
                    ty: Type::Exception,
                    operand: self.instruction(format!(
                        "call ptr @relar_exception_with_cause(ptr {}, ptr {cause})",
                        args[0].value.operand
                    )),
                    small_integer: false,
                }
            }
            Builtin::ExceptionCause => {
                let cause = self.instruction(format!(
                    "call ptr @relar_exception_cause(ptr {})",
                    args[0].value.operand
                ));
                Value {
                    ty: returns,
                    operand: self.instruction(format!("ptrtoint ptr {cause} to i64")),
                    small_integer: false,
                }
            }
            Builtin::ExceptionSuppressContext => {
                let flag = self.instruction(format!(
                    "call i8 @relar_exception_suppress_context(ptr {})",
                    args[0].value.operand
                ));
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!("icmp ne i8 {flag}, 0")),
                    small_integer: false,
                }
            }
            Builtin::Repr => self.value_repr(&args[0].value),
            Builtin::Str => match args[0].value.ty {
                Type::Str | Type::StrLiteral(_) => {
                    return Value {
                        ty: Type::Str,
                        ..self.own_operand(args.into_iter().next().unwrap())
                    };
                }
                Type::StrEnum(_) => self.enum_text(&args[0].value, false),
                Type::Int => self.call("relar_int_str", Type::Str, &[&args[0].value]),
                Type::Float => self.call("relar_float_str", Type::Str, &[&args[0].value]),
                Type::Bytes => self.call("relar_bytes_str", Type::Str, &[&args[0].value]),
                Type::Bool => {
                    let byte = self.instruction(format!("zext i1 {} to i8", args[0].value.operand));
                    Value {
                        ty: Type::Str,
                        operand: self.instruction(format!("call ptr @relar_bool_str(i8 {byte})")),
                        small_integer: false,
                    }
                }
                Type::None => self.call("relar_none_str", Type::Str, &[]),
                Type::Exception | Type::TimeoutExpired => {
                    self.call("relar_exception_message", Type::Str, &[&args[0].value])
                }
                Type::Class(_) => self.value_repr(&args[0].value),
                _ => unreachable!("scalar string conversion was checked"),
            },
            Builtin::Int => match args[0].value.ty {
                Type::Int => return self.own_operand(args.into_iter().next().unwrap()),
                Type::Bool => {
                    let wide =
                        self.instruction(format!("zext i1 {} to i64", args[0].value.operand));
                    let tagged = self.instruction(format!("shl i64 {wide}, 1"));
                    Value {
                        ty: Type::Int,
                        operand: self.instruction(format!("or i64 {tagged}, 1")),
                        small_integer: true,
                    }
                }
                Type::Str => self.call("relar_int_parse", Type::Int, &[&args[0].value]),
                Type::Float => self.call("relar_float_int", Type::Int, &[&args[0].value]),
                _ => unreachable!("int conversion was checked"),
            },
            Builtin::Float => match args[0].value.ty {
                Type::Float => return self.own_operand(args.into_iter().next().unwrap()),
                Type::Int => self.call("relar_int_float", Type::Float, &[&args[0].value]),
                Type::Str => self.call("relar_float_parse", Type::Float, &[&args[0].value]),
                Type::Bytes => self.call("relar_bytes_float", Type::Float, &[&args[0].value]),
                Type::Bool => Value {
                    ty: Type::Float,
                    operand: self
                        .instruction(format!("uitofp i1 {} to double", args[0].value.operand)),
                    small_integer: false,
                },
                _ => unreachable!("float conversion was checked"),
            },
            Builtin::Len if matches!(args[0].value.ty, Type::Tuple(_)) => {
                small(self.module.types.fields(args[0].value.ty).unwrap().len() as i64)
            }
            Builtin::Len => {
                if let ExprKind::Local(slot) = expressions[0].kind
                    && let Some(length) = self.snapshot_length(slot, &args[0].value)
                {
                    length
                } else {
                    self.call(
                        match args[0].value.ty {
                            Type::Str => "relar_str_len",
                            Type::Bytes => "relar_bytes_len",
                            Type::ListInt => "relar_list_len",
                            Type::List(_) => "relar_seq_len_fast",
                            Type::Dict(_) => "relar_dict_len",
                            Type::Set(_) => "relar_set_len",
                            _ => unreachable!("len input was checked"),
                        },
                        Type::Int,
                        &[&args[0].value],
                    )
                }
            }
            Builtin::ListGet => self.get_item(
                &args[0].value,
                &args[1].value,
                returns,
                self.nonnegative_index(&expressions[1]),
            ),
            Builtin::SequenceGetInBounds => {
                debug_assert!(matches!(
                    expressions[1].kind,
                    ExprKind::Local(_) | ExprKind::Int(_)
                ));
                let ExprKind::Local(slot) = expressions[0].kind else {
                    unreachable!("proven sequence receivers are private locals")
                };
                let value =
                    self.borrow_sequence_element(slot, &args[0].value, &args[1].value, returns);
                // Acquire before argument cleanup, target replacement, or the
                // next iterator pull can remove the source element's owner.
                self.retain(&value);
                value
            }
            Builtin::ListSum => self.call(
                "relar_list_sum",
                Type::Int,
                &[&args[0].value, &args[1].value],
            ),
            Builtin::ListSliceSum => self.call(
                "relar_list_slice_sum",
                Type::Int,
                &args.iter().map(|arg| &arg.value).collect::<Vec<_>>(),
            ),
            Builtin::ListGetI32
            | Builtin::ListGetBounded { .. }
            | Builtin::ListGetBoundedInBounds { .. } => {
                let (min, max) = match builtin {
                    Builtin::ListGetBounded { min, max }
                    | Builtin::ListGetBoundedInBounds { min, max } => (*min, *max),
                    _ => (i64::from(i32::MIN), i64::from(i32::MAX)),
                };
                debug_assert!(min <= max && min >= -(1_i64 << 62) && max < (1_i64 << 62));
                let in_bounds = matches!(builtin, Builtin::ListGetBoundedInBounds { .. });
                let nonnegative = in_bounds || self.nonnegative_index(&expressions[1]);
                let mut index = args[1].value.clone();
                index.small_integer |= in_bounds;
                let index = self.index_word(&index, nonnegative);
                let name = if in_bounds {
                    "relar_list_borrow_index_proven"
                } else if nonnegative {
                    "relar_list_borrow_index"
                } else {
                    "relar_list_get"
                };
                let call = format!(
                    "call i64 @{name}(ptr {}, i64 {index})",
                    args[0].value.operand
                );
                let word = if in_bounds {
                    self.instruction(call)
                } else {
                    self.checked_int_list_word(call)
                };
                let tag = self.instruction(format!("and i64 {word}, 1"));
                let tagged = self.instruction(format!("icmp eq i64 {tag}, 1"));
                self.line(format!("call void @llvm.assume(i1 {tagged})"));
                let encoded_min = min * 2 + 1;
                let width = ((i128::from(max) - i128::from(min)) * 2) as u64;
                let shifted = self.instruction(format!("sub i64 {word}, {encoded_min}"));
                let bounded = self.instruction(format!("icmp ule i64 {shifted}, {width}"));
                self.line(format!("call void @llvm.assume(i1 {bounded})"));
                Value {
                    ty: Type::Int,
                    operand: word,
                    small_integer: true,
                }
            }
            Builtin::IntBounded { min, max } => {
                debug_assert!(min <= max && *min >= -(1_i64 << 62) && *max < (1_i64 << 62));
                debug_assert!(matches!(expressions[0].kind, ExprKind::Local(_)));
                // Range analysis already uses the checked induction proof to
                // select native arithmetic and old-value cleanup. Repeating
                // it as llvm.assume adds a use of the loop-carried subtotal
                // that prevents LLVM from recognizing an ordinary reduction.
                Value {
                    ty: Type::Int,
                    operand: args[0].value.operand.clone(),
                    small_integer: true,
                }
            }
            Builtin::ListAllI32 => {
                let result = self.instruction(format!(
                    "call i8 @relar_list_all_i32(ptr {}, i64 {}, i64 {}, i64 {})",
                    args[0].value.operand,
                    args[1].value.operand,
                    args[2].value.operand,
                    args[3].value.operand
                ));
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!("icmp ne i8 {result}, 0")),
                    small_integer: false,
                }
            }
            Builtin::ListDistinct => Value {
                ty: Type::Bool,
                operand: self.instruction(format!(
                    "icmp ne ptr {}, {}",
                    args[0].value.operand, args[1].value.operand
                )),
                small_integer: false,
            },
            Builtin::ListReserveRange => {
                let count = self.index_word(&args[4].value, true);
                self.line(format!(
                    "call void @relar_list_reserve_range(ptr {}, i64 {}, i64 {}, i64 {}, i64 {count})",
                    args[0].value.operand,
                    args[1].value.operand,
                    args[2].value.operand,
                    args[3].value.operand
                ));
                none()
            }
            Builtin::StringGet => self.call(
                "relar_str_get",
                Type::Str,
                &[&args[0].value, &args[1].value],
            ),
            Builtin::BytesGet => {
                // Successful bytes are canonical tagged integers in 1..=511;
                // zero uniquely signals failure, so no pending-state poll is needed.
                let nonnegative = self.nonnegative_index(&expressions[1]);
                let index = self.index_word(&args[1].value, nonnegative);
                let function = if nonnegative {
                    "relar_bytes_get_index"
                } else {
                    "relar_bytes_get"
                };
                let operand = self.instruction(format!(
                    "call i64 @{function}(ptr {}, i64 {index})",
                    args[0].value.operand
                ));
                let failed = self.instruction(format!("icmp eq i64 {operand}, 0"));
                self.check_failed(&failed);
                let bounded = self.instruction(format!("icmp ule i64 {operand}, 511"));
                self.line(format!("call void @llvm.assume(i1 {bounded})"));
                Value {
                    ty: Type::Int,
                    operand,
                    small_integer: true,
                }
            }
            Builtin::BytesIter => self.call("relar_bytes_iter", Type::Iterator, &[&args[0].value]),
            Builtin::StringIter => self.call("relar_str_iter", Type::Iterator, &[&args[0].value]),
            Builtin::StringReverseIter => {
                self.call("relar_str_reverse_iter", Type::Iterator, &[&args[0].value])
            }
            Builtin::StringIntList => {
                self.call("relar_str_int_list", Type::ListInt, &[&args[0].value])
            }
            Builtin::Slice => self.call(
                match returns {
                    Type::Str => "relar_str_slice",
                    Type::Bytes => "relar_bytes_slice",
                    Type::ListInt => "relar_list_slice",
                    Type::List(_) => "relar_seq_slice",
                    _ => unreachable!("slice receiver was checked"),
                },
                returns,
                &args.iter().map(|arg| &arg.value).collect::<Vec<_>>(),
            ),
            Builtin::DictGet => self.get_item(&args[0].value, &args[1].value, returns, false),
            Builtin::DictGatherStrInt => self.call(
                "relar_dict_gather_str_int",
                Type::ListInt,
                &[&args[0].value, &args[1].value],
            ),
            Builtin::DictGatherBytesInt => self.call(
                "relar_dict_gather_bytes_int",
                Type::ListInt,
                &[&args[0].value, &args[1].value],
            ),
            Builtin::DictGetDefault => {
                let key = self.word(&args[1].value);
                let default = self.word(&args[2].value);
                let word = self.instruction(format!(
                    "call i64 @relar_dict_get_default(ptr {}, i64 {key}, i64 {default})",
                    args[0].value.operand
                ));
                self.unpack_word(word, returns)
            }
            Builtin::DictTryGet(stored) => self.dict_try_get(*stored, &args, returns),
            Builtin::DictContains | Builtin::SetContains => {
                let key = self.word(&args[0].value);
                let family = if matches!(builtin, Builtin::SetContains) {
                    "set"
                } else {
                    "dict"
                };
                let byte = self.instruction(format!(
                    "call i8 @relar_{family}_contains(ptr {}, i64 {key})",
                    args[1].value.operand
                ));
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!("icmp ne i8 {byte}, 0")),
                    small_integer: false,
                }
            }
            Builtin::DictDelete => {
                let key = self.word(&args[1].value);
                self.checked_line(format!(
                    "call void @relar_dict_delete(ptr {}, i64 {key})",
                    args[0].value.operand
                ));
                none()
            }
            Builtin::DictIter(items) => Value {
                ty: Type::Iterator,
                operand: self.instruction(format!(
                    "call ptr @relar_dict_iter(ptr {}, i32 {})",
                    args[0].value.operand,
                    u8::from(*items)
                )),
                small_integer: false,
            },
            Builtin::SetAdd | Builtin::SetDiscard => {
                self.set_member(
                    &args[0].value,
                    &args[1].value,
                    matches!(builtin, Builtin::SetAdd),
                );
                none()
            }
            Builtin::SetIter => self.call("relar_set_iter", Type::Iterator, &[&args[0].value]),
            Builtin::Append => {
                self.append(&args[0].value, &args[1].value);
                none()
            }
            Builtin::Pop => {
                if args[0].value.ty == Type::ListInt {
                    let word = self.checked_int_list_word(format!(
                        "call i64 @relar_list_pop(ptr {})",
                        args[0].value.operand
                    ));
                    self.unpack_word(word, Type::Int)
                } else {
                    let word = self.checked_instruction(format!(
                        "call i64 @relar_seq_pop(ptr {})",
                        args[0].value.operand
                    ));
                    self.unpack_word(word, returns)
                }
            }
            Builtin::Split => self.call("relar_str_split_whitespace", returns, &[&args[0].value]),
            Builtin::SplitLines => self.call("relar_str_splitlines", returns, &[&args[0].value]),
            Builtin::RStrip => {
                let chars = if let Some(arg) = args.get(1) {
                    let word = self.word(&arg.value);
                    self.decode_word(word, Type::Str).operand
                } else {
                    "null".to_owned()
                };
                Value {
                    ty: Type::Str,
                    operand: self.instruction(format!(
                        "call ptr @relar_str_rstrip(ptr {}, ptr {chars})",
                        args[0].value.operand,
                    )),
                    small_integer: false,
                }
            }
            Builtin::IsNone => Value {
                ty: Type::Bool,
                operand: match args[0].value.ty {
                    Type::Optional(_) => {
                        self.instruction(format!("icmp eq i64 {}, 0", args[0].value.operand))
                    }
                    Type::None => "true".to_owned(),
                    _ => "false".to_owned(),
                },
                small_integer: false,
            },
            Builtin::Print => {
                match args[0].value.ty {
                    Type::Int => self.void_call("relar_print_int", &[&args[0].value]),
                    Type::Float => self.void_call("relar_print_float", &[&args[0].value]),
                    Type::Str | Type::StrLiteral(_) => {
                        self.void_call("relar_print_str", &[&args[0].value])
                    }
                    Type::StrEnum(_) => {
                        let text = self.enum_text(&args[0].value, false);
                        let text = self.owned(text);
                        self.void_call("relar_print_str", &[&text]);
                        self.release(&text);
                    }
                    Type::Bytes => self.void_call("relar_print_bytes", &[&args[0].value]),
                    Type::Bool => {
                        let byte =
                            self.instruction(format!("zext i1 {} to i8", args[0].value.operand));
                        self.checked_line(format!("call void @relar_print_bool(i8 {byte})"));
                    }
                    Type::None => self.void_call("relar_print_none", &[]),
                    Type::ListInt
                    | Type::Class(_)
                    | Type::List(_)
                    | Type::Dict(_)
                    | Type::Set(_)
                    | Type::Iterator
                    | Type::Exception
                    | Type::TimeoutExpired
                    | Type::Path
                    | Type::TextIO
                    | Type::RegexPattern
                    | Type::RegexMatch
                    | Type::BinaryIO
                    | Type::CompletedProcess
                    | Type::Tuple(_)
                    | Type::Optional(_)
                    | Type::Callable(_)
                    | Type::Union(_) => {
                        unreachable!("container printing is rejected")
                    }
                }
                none()
            }
        };
        for arg in &args {
            self.release_operand(arg);
        }
        result
    }

    /// Uses word-sized scratch storage only on the successful lookup edge. A
    /// found nonoptional payload transfers into its nullable representation;
    /// existing optional/None storage stays flattened. Defaults were evaluated
    /// eagerly, and the missing edge acquires its own owner before argument
    /// cleanup can release the supplied fallback.
    fn dict_try_get(&mut self, stored: Type, args: &[Operand], returns: Type) -> Value {
        let key = self.word(&args[1].value);
        let byte = self.instruction(format!(
            "call i8 @relar_dict_try_get(ptr {}, i64 {key}, ptr %mapping_key_word)",
            args[0].value.operand
        ));
        let found = self.instruction(format!("icmp ne i8 {byte}, 0"));
        let (present, absent, end) = (self.label(), self.label(), self.label());
        self.branch(&found, &present, &absent);
        self.start(&present);
        let word = self.instruction("load i64, ptr %mapping_key_word");
        let value = self.unpack_word(word, stored);
        let value = if matches!(stored, Type::Optional(_) | Type::None) {
            debug_assert_eq!(stored, returns);
            value
        } else {
            debug_assert!(matches!(returns, Type::Optional(_)));
            self.present(value, returns)
        };
        self.forget(&value);
        let present_end = self.current.clone();
        self.jump(&end);
        self.start(&absent);
        let default = if let Some(default) = args.get(2) {
            debug_assert_eq!(default.value.ty, returns);
            self.retain(&default.value);
            default.value.operand.clone()
        } else {
            returns.initial().to_owned()
        };
        self.jump(&end);
        self.start(&end);
        Value {
            ty: returns,
            operand: self.instruction(format!(
                "phi {} [ {}, %{present_end} ], [ {default}, %{absent} ]",
                returns.llvm(),
                value.operand
            )),
            small_integer: false,
        }
    }

    /// The runtime transfers independent owned words only on success. Output
    /// locals have distinct slots and may be narrower than a word, so scratch
    /// values are converted before publication. Exhaustion never reads scratch
    /// or replaces an output owner; nested loops can safely reuse both scratch
    /// words after this synchronous conversion.
    fn iter_next(&mut self, iterator: usize, key: usize, value: Option<usize>, ready: usize) {
        debug_assert_eq!(self.locals[iterator], Type::Iterator);
        debug_assert_eq!(self.locals[ready], Type::Bool);
        debug_assert_ne!(iterator, key);
        debug_assert_ne!(iterator, ready);
        debug_assert_ne!(key, ready);
        if let Some(value) = value {
            debug_assert_ne!(iterator, value);
            debug_assert_ne!(key, value);
            debug_assert_ne!(ready, value);
        }
        let iterator = self.borrow_slot(iterator);
        let value_output = if value.is_some() {
            "%mapping_value_word"
        } else {
            "null"
        };
        let byte = self.checked_instruction(format!(
            "call i8 @relar_mapping_iter_next(ptr {}, ptr %mapping_key_word, ptr {value_output})",
            iterator.operand
        ));
        let ready_value = Value {
            ty: Type::Bool,
            operand: self.instruction(format!("icmp ne i8 {byte}, 0")),
            small_integer: false,
        };
        self.store(ready, &ready_value);
        let (success, end) = (self.label(), self.label());
        self.branch(&ready_value.operand, &success, &end);
        self.start(&success);
        let word = self.instruction("load i64, ptr %mapping_key_word");
        let key_value = self.unpack_word(word, self.locals[key]);
        self.store(key, &key_value);
        if let Some(value) = value {
            let word = self.instruction("load i64, ptr %mapping_value_word");
            let value_result = self.unpack_word(word, self.locals[value]);
            self.store(value, &value_result);
        }
        self.jump(&end);
        self.start(&end);
    }

    /// Converts a typed value to the runtime's word representation without
    /// changing ownership. The slot's representation tag controls reclamation.
    fn word(&mut self, value: &Value) -> String {
        match value.ty {
            Type::Int | Type::StrEnum(_) | Type::Optional(_) | Type::Union(_) => {
                value.operand.clone()
            }
            Type::Float => self.instruction(format!("bitcast double {} to i64", value.operand)),
            Type::Bool => self.instruction(format!("zext i1 {} to i64", value.operand)),
            Type::None => "0".to_owned(),
            Type::Str
            | Type::StrLiteral(_)
            | Type::Bytes
            | Type::ListInt
            | Type::Class(_)
            | Type::Callable(_)
            | Type::List(_)
            | Type::Dict(_)
            | Type::Set(_)
            | Type::Iterator
            | Type::Exception
            | Type::TimeoutExpired
            | Type::Path
            | Type::TextIO
            | Type::BinaryIO
            | Type::RegexPattern
            | Type::RegexMatch
            | Type::CompletedProcess
            | Type::Tuple(_) => self.instruction(format!("ptrtoint ptr {} to i64", value.operand)),
        }
    }

    /// Reconstitutes the exact type established by the contract checker. Runtime
    /// getters transfer an owned reference before releasing their container borrow.
    fn unpack_word(&mut self, word: String, ty: Type) -> Value {
        let value = self.decode_word(word, ty);
        self.owned(value)
    }

    /// Changes only the compiler view of a word, leaving its ownership unchanged.
    fn decode_word(&mut self, word: String, ty: Type) -> Value {
        let operand = match ty {
            Type::Int | Type::StrEnum(_) | Type::Optional(_) | Type::Union(_) => word,
            Type::Float => self.instruction(format!("bitcast i64 {word} to double")),
            Type::Bool => self.instruction(format!("trunc i64 {word} to i1")),
            Type::None => "0".to_owned(),
            Type::Str
            | Type::StrLiteral(_)
            | Type::Bytes
            | Type::ListInt
            | Type::Class(_)
            | Type::Callable(_)
            | Type::List(_)
            | Type::Dict(_)
            | Type::Set(_)
            | Type::Iterator
            | Type::Exception
            | Type::TimeoutExpired
            | Type::Path
            | Type::TextIO
            | Type::BinaryIO
            | Type::RegexPattern
            | Type::RegexMatch
            | Type::CompletedProcess
            | Type::Tuple(_) => self.instruction(format!("inttoptr i64 {word} to ptr")),
        };
        Value {
            ty,
            operand,
            small_integer: false,
        }
    }

    /// Moves an owned payload into a nullable word. Canonical integers and live
    /// pointers never use zero; booleans reserve it by encoding False/True as 1/2.
    /// Floats preserve all binary64 patterns in a separately owned allocation.
    fn present(&mut self, value: Value, ty: Type) -> Value {
        debug_assert_eq!(self.module.types.payload(ty), Some(value.ty));
        if value.ty == Type::Float {
            return Value {
                ty,
                operand: self.instruction(format!(
                    "call i64 @relar_float_box(double {})",
                    value.operand
                )),
                small_integer: false,
            };
        }
        let word = self.word(&value);
        let operand = if value.ty == Type::Bool {
            self.instruction(format!("add i64 {word}, 1"))
        } else {
            word
        };
        self.forget(&value);
        Value {
            ty,
            operand,
            small_integer: false,
        }
    }

    /// The zeroed payload is safe to destroy before every initializer completes.
    /// The checked constructor cannot expose a partially initialized record.
    fn record(&mut self, ty: Type) -> Value {
        let value = Value {
            ty,
            operand: self.instruction(format!(
                "call ptr @relar_record_new(ptr @layout_{})",
                record_name(ty)
            )),
            small_integer: false,
        };
        self.owned(value)
    }

    /// An unaliased tuple local can reuse its allocation on the next iteration.
    /// Finish every RHS effect first, including rebinding the destination, and
    /// root each field across the remaining evaluations and all replacements.
    fn assign_loop_tuple(&mut self, slot: usize, fields: &[Expr]) {
        let fields: Vec<_> = fields.iter().map(|field| self.expr(field)).collect();
        let previous = self.borrow_slot(slot);
        let empty = self.instruction(format!("icmp eq ptr {}, null", previous.operand));
        let allocate = self.label();
        let cached = self.label();
        let write = self.label();
        self.branch(&empty, &allocate, &cached);
        self.start(&allocate);
        let fresh = self.instruction(format!(
            "call ptr @relar_record_new(ptr @layout_{})",
            record_name(previous.ty)
        ));
        self.jump(&write);
        self.start(&cached);
        let replacement = self.instruction(format!(
            "call ptr @relar_record_cached_result(ptr {})",
            previous.operand
        ));
        self.jump(&write);
        self.start(&write);
        let record = Value {
            ty: previous.ty,
            operand: self.instruction(format!(
                "phi ptr [{fresh}, %{allocate}], [{replacement}, %{cached}]"
            )),
            small_integer: false,
        };
        for (index, field) in fields.iter().enumerate() {
            self.forget(field);
            self.replace_field(&record, index, field);
        }
        self.store(slot, &record);
    }

    fn field_pointer(&mut self, receiver: &Value, field: usize) -> String {
        self.instruction(format!(
            "getelementptr inbounds %{}, ptr {}, i32 0, i32 {field}",
            record_name(receiver.ty),
            receiver.operand
        ))
    }

    /// Reads a field without acquiring ownership. A not-yet-initialized field may
    /// be read only for replacement cleanup, whose ownership operation accepts zero.
    fn field_value(&mut self, pointer: &str, ty: Type) -> Value {
        let operand = self.instruction(format!("load {}, ptr {pointer}", storage_type(ty)));
        if matches!(storage_type(ty), "ptr" | "double") {
            Value {
                ty,
                operand,
                small_integer: false,
            }
        } else {
            self.decode_word(operand, ty)
        }
    }

    /// Exact layouts and complete initialization prove direct access safe. The
    /// acquired value remains alive if later evaluation replaces the source field.
    fn get_field(&mut self, receiver: &Value, field: usize, ty: Type) -> Value {
        let pointer = self.field_pointer(receiver, field);
        let value = self.field_value(&pointer, ty);
        self.retain(&value);
        self.owned(value)
    }

    /// Acquires before publishing and releases afterward, including self-aliasing
    /// replacement. Zero-initialized owning fields are valid cleanup sentinels.
    fn set_field(&mut self, receiver: &Value, field: usize, value: &Value) {
        self.retain(value);
        self.replace_field(receiver, field, value);
    }

    /// Transfers an already-owned field and then releases the previous word.
    fn replace_field(&mut self, receiver: &Value, field: usize, value: &Value) {
        let pointer = self.field_pointer(receiver, field);
        let old = self.field_value(&pointer, value.ty);
        let operand = if matches!(storage_type(value.ty), "ptr" | "double") {
            value.operand.clone()
        } else {
            self.word(value)
        };
        self.line(format!(
            "store {} {operand}, ptr {pointer}",
            storage_type(value.ty)
        ));
        if old.ty == Type::Int {
            // This raw field word can still be the zero initialization sentinel.
            // It has not been registered as an owned canonical integer.
            self.void_call("release_int", &[&old]);
        } else {
            self.release(&old);
        }
    }

    fn append(&mut self, list: &Value, value: &Value) {
        if list.ty == Type::ListInt {
            self.void_call("relar_list_append", &[list, value]);
        } else {
            let word = self.word(value);
            self.line(format!(
                "call void @relar_seq_append(ptr {}, i64 {word}, i32 {})",
                list.operand,
                representation_kind(value.ty, &self.module.types)
            ));
        }
    }

    /// Passes a rooted canonical int/str/bytes key without index decoding. The runtime
    /// acquires a newly stored element and drops removed owners before returning;
    /// the caller keeps both operands rooted throughout either operation.
    fn set_member(&mut self, set: &Value, value: &Value, insert: bool) {
        debug_assert!(matches!(set.ty, Type::Set(_)));
        debug_assert!(self.module.types.hashable_key(value.ty));
        let word = self.word(value);
        let operation = if insert { "add" } else { "discard" };
        self.line(format!(
            "call void @relar_set_{operation}(ptr {}, i64 {word})",
            set.operand
        ));
    }

    fn nonnegative_index(&self, expression: &Expr) -> bool {
        self.ranges
            .integer(expression)
            .is_some_and(|range| range.min >= 0)
    }

    /// Fuses a copy only within the same rooted list local. Both index reads
    /// must be immediate literals or stable locals, so evaluating the target
    /// index before the runtime's source bounds check cannot introduce effects
    /// or failures. The runtime checks and clones the source before validating
    /// and replacing the destination, preserving self-copy and BigInt owners.
    fn copy_item(&mut self, receiver: &Expr, index: &Expr, value: &Expr) -> bool {
        let ExprKind::Local(target) = &receiver.kind else {
            return false;
        };
        if receiver.ty != Type::ListInt || value.ty != Type::Int {
            return false;
        }
        let ExprKind::Builtin(Builtin::ListGet, args) = &value.kind else {
            return false;
        };
        let [source_receiver, source] = args.as_slice() else {
            return false;
        };
        if source_receiver.ty != Type::ListInt
            || !matches!(&source_receiver.kind, ExprKind::Local(slot) if slot == target)
            || ![source, index].into_iter().all(|expression| {
                expression.ty == Type::Int
                    && matches!(expression.kind, ExprKind::Local(_) | ExprKind::Int(_))
                    && self.nonnegative_index(expression)
            })
        {
            return false;
        }
        let list = self.operand(source_receiver);
        let source = self.operand(source);
        let destination = self.operand(index);
        let from = self.index_word(&source.value, true);
        let to = self.index_word(&destination.value, true);
        self.checked_int_list_status(format!(
            "call i8 @relar_list_copy_index(ptr {}, i64 {to}, i64 {from})",
            list.value.operand
        ));
        self.release_operand(&destination);
        self.release_operand(&source);
        self.release_operand(&list);
        true
    }

    /// A proved nonnegative immediate can cross the specialized ABI as an
    /// exact native index. Its separate current-size check is retained unless
    /// the caller also carries the canonical loop's relational bounds proof.
    fn index_word(&mut self, index: &Value, nonnegative: bool) -> String {
        if nonnegative {
            debug_assert!(index.small_integer);
            // The range proof fixes the tag and sign bits too. Publish them so
            // LLVM can eliminate loop-carried integer ownership checks.
            let tag = self.instruction(format!("and i64 {}, {}", index.operand, i64::MIN | 1));
            let canonical = self.instruction(format!("icmp eq i64 {tag}, 1"));
            self.line(format!("call void @llvm.assume(i1 {canonical})"));
            self.instruction(format!("lshr i64 {}, 1", index.operand))
        } else {
            index.operand.clone()
        }
    }

    fn get_item(&mut self, list: &Value, index: &Value, returns: Type, nonnegative: bool) -> Value {
        if matches!(list.ty, Type::Dict(_)) {
            let key = self.word(index);
            let word = self.checked_instruction(format!(
                "call i64 @relar_dict_get(ptr {}, i64 {key})",
                list.operand
            ));
            return self.unpack_word(word, returns);
        }
        let family = if list.ty == Type::ListInt {
            "list"
        } else {
            "seq"
        };
        let suffix = if nonnegative { "_index" } else { "" };
        let index = self.index_word(index, nonnegative);
        let instruction = format!(
            "call i64 @relar_{family}_get{suffix}(ptr {}, i64 {index})",
            list.operand
        );
        let word = if list.ty == Type::ListInt {
            self.checked_int_list_word(instruction)
        } else {
            self.checked_instruction(instruction)
        };
        self.unpack_word(word, returns)
    }

    fn set_item(&mut self, list: &Value, index: &Value, value: &Value, nonnegative: bool) {
        if matches!(list.ty, Type::Dict(_)) {
            let key = self.word(index);
            let word = self.word(value);
            self.line(format!(
                "call void @relar_dict_set(ptr {}, i64 {key}, i64 {word})",
                list.operand
            ));
            return;
        }
        let family = if list.ty == Type::ListInt {
            "list"
        } else {
            "seq"
        };
        let suffix = if nonnegative { "_index" } else { "" };
        let index = self.index_word(index, nonnegative);
        let word = self.word(value);
        if list.ty == Type::ListInt {
            self.checked_int_list_status(format!(
                "call i8 @relar_list_set{suffix}(ptr {}, i64 {index}, i64 {word})",
                list.operand
            ));
        } else {
            self.checked_line(format!(
                "call void @relar_{family}_set{suffix}(ptr {}, i64 {index}, i64 {word}, i32 {})",
                list.operand,
                representation_kind(value.ty, &self.module.types)
            ));
        }
    }
}

/// Proves only that integer evaluation cannot mutate a list or invoke user
/// code. Allocation, arithmetic errors, and current bounds checks remain in
/// their original order. Both complete comparison operands must pass before
/// borrowing any element; integer temporaries have no user-defined finalizers.
fn read_only_integer(expression: &Expr) -> bool {
    if expression.ty != Type::Int {
        return false;
    }
    match &expression.kind {
        ExprKind::Int(_) | ExprKind::Local(_) => true,
        ExprKind::Binary(op, left, right) => {
            matches!(
                op,
                Binary::Add
                    | Binary::Subtract
                    | Binary::Multiply
                    | Binary::FloorDivide
                    | Binary::Modulo
            ) && read_only_integer(left)
                && read_only_integer(right)
        }
        ExprKind::Negate(value) => read_only_integer(value),
        ExprKind::Builtin(Builtin::ListGet, args) => {
            matches!(args.as_slice(), [receiver, index]
                if receiver.ty == Type::ListInt
                    && matches!(receiver.kind, ExprKind::Local(_))
                    && read_only_integer(index))
        }
        _ => false,
    }
}

/// Rejects every local replacement hidden inside an operand. Statement regions
/// and expression scopes are each visited once, so nested scopes do not cause
/// repeated recursive walks of their bodies.
fn expression_preserves_local(expression: &Expr, slot: usize, locals: usize) -> bool {
    fn expression_writes(expression: &Expr, written: &mut [bool]) {
        match &expression.kind {
            ExprKind::Scope { body, value, clear } => {
                crate::list_specialization::mark_writes(body, written);
                for &slot in clear {
                    written[slot] = true;
                }
                crate::ir::walk_expressions(body, &mut |value| match &value.kind {
                    ExprKind::Scope { body, clear, .. } => {
                        crate::list_specialization::mark_writes(body, written);
                        for &slot in clear {
                            written[slot] = true;
                        }
                    }
                    ExprKind::Bind(slot, _) => written[*slot] = true,
                    _ => {}
                });
                expression_writes(value, written);
            }
            ExprKind::Bind(slot, value) => {
                written[*slot] = true;
                expression_writes(value, written);
            }
            ExprKind::List(values)
            | ExprKind::Tuple(values)
            | ExprKind::Set { values, .. }
            | ExprKind::Compare(values, _)
            | ExprKind::Boolean(_, values)
            | ExprKind::Builtin(_, values) => {
                for value in values {
                    expression_writes(value, written);
                }
            }
            ExprKind::Call(_, args) | ExprKind::Construct { args, .. } => {
                for value in &args.values {
                    expression_writes(value, written);
                }
            }
            ExprKind::Callable {
                receiver: Some(receiver),
                ..
            } => expression_writes(receiver, written),
            ExprKind::Invoke { callable, args } => {
                expression_writes(callable, written);
                for value in &args.values {
                    expression_writes(value, written);
                }
            }
            ExprKind::Dict { entries, .. } => {
                for (key, value) in entries {
                    expression_writes(key, written);
                    expression_writes(value, written);
                }
            }
            ExprKind::Binary(_, left, right) | ExprKind::BooleanValue(_, left, right) => {
                expression_writes(left, written);
                expression_writes(right, written);
            }
            ExprKind::Conditional(test, yes, no) => {
                expression_writes(test, written);
                expression_writes(yes, written);
                expression_writes(no, written);
            }
            ExprKind::Present(value)
            | ExprKind::Absent(value)
            | ExprKind::Unwrap(value)
            | ExprKind::Not(value)
            | ExprKind::Negate(value)
            | ExprKind::Field(value, _) => expression_writes(value, written),
            ExprKind::Exception {
                message: Some(message),
                ..
            } => expression_writes(message, written),
            _ => {}
        }
    }
    let mut written = vec![false; locals];
    expression_writes(expression, &mut written);
    !written[slot]
}

/// Flattens only exact string additions, preserving both source nesting shapes'
/// leaf order without evaluating through calls or other expression boundaries.
fn concat_leaves(expression: &Expr) -> Vec<&Expr> {
    fn visit<'a>(expression: &'a Expr, output: &mut Vec<&'a Expr>) {
        if let ExprKind::Binary(Binary::Concat, left, right) = &expression.kind {
            visit(left, output);
            visit(right, output);
        } else {
            debug_assert_eq!(expression.ty, Type::Str);
            output.push(expression);
        }
    }

    let mut result = Vec::new();
    visit(expression, &mut result);
    result
}

/// The checked IR publishes each module binding once and forbids later rebinding.
/// Retaining only direct literal initializers avoids evaluating any user code or
/// assuming a mutable container's contents are constant.
fn literal_globals(module: &Module) -> Vec<Option<String>> {
    let mut strings = vec![None; module.globals.len()];
    for function in module.functions.iter().chain([&module.entry]) {
        crate::ir::walk_expressions(&function.body, &mut |expression| {
            if let ExprKind::Builtin(Builtin::GlobalSet(slot), args) = &expression.kind
                && let [value] = args.as_slice()
                && let ExprKind::Str(value) = &value.kind
            {
                strings[*slot] = Some(value.clone());
            }
        });
    }
    strings
}

/// Only the sealed integer conversion can use the eager scratch formatter.
fn integer_string_argument(expression: &Expr) -> Option<&Expr> {
    if let ExprKind::Builtin(Builtin::Str, args) = &expression.kind
        && let [value] = args.as_slice()
        && value.ty == Type::Int
    {
        Some(value)
    } else {
        None
    }
}

/// Must match the descriptor kinds in the runtime. Nullable words preserve the
/// payload's ownership operation, with bit 8 allowing the absent zero value.
fn representation_kind(ty: Type, types: &Types) -> u32 {
    match ty {
        Type::None => 0,
        Type::Bool => 1,
        Type::Int => 2,
        Type::Float => 7,
        Type::Str | Type::StrLiteral(_) => 3,
        Type::StrEnum(_) => 10,
        Type::Bytes => 8,
        Type::Union(_) => 9,
        Type::ListInt => 4,
        Type::Class(_) | Type::Tuple(_) | Type::Callable(_) | Type::CompletedProcess => 6,
        Type::List(_)
        | Type::Dict(_)
        | Type::Set(_)
        | Type::Iterator
        | Type::Exception
        | Type::TimeoutExpired
        | Type::Path
        | Type::TextIO
        | Type::RegexPattern
        | Type::RegexMatch
        | Type::BinaryIO => 5,
        Type::Optional(_) => 256 | representation_kind(types.payload(ty).unwrap(), types),
    }
}

fn record_name(ty: Type) -> String {
    match ty {
        Type::Class(id) => format!("class_{id}"),
        Type::Tuple(id) => format!("tuple_{id}"),
        Type::Callable(_) => "callable".to_owned(),
        Type::CompletedProcess => "completed_process".to_owned(),
        _ => unreachable!("only fixed record types have layout names"),
    }
}

/// All fields occupy one aligned word. Pointers and doubles retain their LLVM
/// types; booleans/None use word storage so descriptor offsets stay uniform
/// within the runtime's existing 64-bit ABI.
fn storage_type(ty: Type) -> &'static str {
    match ty.llvm() {
        "ptr" => "ptr",
        "double" => "double",
        _ => "i64",
    }
}

/// Emits concrete field layouts and static destruction descriptors. No per-field
/// runtime tag or presence bitmap is needed: owning fields reserve zero until
/// initialization, and their exact kinds are shared by every instance.
fn record_layouts(module: &Module) -> String {
    let mut output = String::new();
    let classes = module
        .class_fields
        .iter()
        .enumerate()
        .map(|(id, fields)| (Type::Class(id), fields.clone()));
    let process = (
        Type::CompletedProcess,
        vec![
            module.types.list(Type::Str),
            Type::Int,
            Type::Str,
            Type::Str,
        ],
    );
    for (ty, fields) in classes
        .chain(module.types.tuples())
        .chain(std::iter::once(process))
    {
        let name = record_name(ty);
        let storage = fields
            .iter()
            .map(|&ty| storage_type(ty))
            .collect::<Vec<_>>()
            .join(", ");
        let kinds = fields
            .iter()
            .map(|&ty| format!("i32 {}", representation_kind(ty, &module.types)))
            .collect::<Vec<_>>()
            .join(", ");
        let count = fields.len();
        writeln!(output, "%{name} = type {{ {storage} }}").unwrap();
        writeln!(
            output,
            "@kinds_{name} = private unnamed_addr constant [{count} x i32] [{kinds}]"
        )
        .unwrap();
        let semantics = records::semantics(module, ty, &name, &mut output);
        writeln!(
            output,
            "@layout_{name} = private constant {{ i64, ptr, ptr }} {{ i64 {count}, ptr @kinds_{name}, ptr {semantics} }}"
        )
        .unwrap();
    }
    output
}

fn small(value: i64) -> Value {
    Value {
        ty: Type::Int,
        operand: (((value as u64) << 1) | 1).to_string(),
        small_integer: true,
    }
}

fn none() -> Value {
    Value {
        ty: Type::None,
        operand: "0".to_owned(),
        small_integer: false,
    }
}

/// This list follows the runtime's pending-error contract. Proven integer
/// arithmetic, ownership operations, lookup-with-default, lengths and hashing
/// remain infallible; allocation failure and invalid ABI use are fatal.
fn fallible(name: &str) -> bool {
    name.starts_with("fn_")
        || matches!(
            name,
            "int_floordiv"
                | "int_mod"
                | "int_lshift"
                | "int_rshift"
                | "relar_flush_stdout"
                | "relar_subprocess_run"
                | "relar_subprocess_run_timeout"
                | "relar_environ_copy"
                | "relar_subprocess_check"
                | "relar_subprocess_error_field"
                | "relar_print_int"
                | "relar_print_float"
                | "relar_print_str"
                | "relar_print_bytes"
                | "relar_bytes_count"
                | "relar_bytes_list"
                | "relar_bytes_get"
                | "relar_bytes_get_index"
                | "relar_bytes_slice"
                | "relar_bytes_concat"
                | "relar_bytes_repeat"
                | "relar_bytes_decode"
                | "relar_bytes_validate_utf8"
                | "relar_binary_open"
                | "relar_binary_read"
                | "relar_binary_read_size"
                | "relar_binary_readline"
                | "relar_gzip_open"
                | "relar_binary_write"
                | "relar_print_bool"
                | "relar_print_none"
                | "relar_input"
                | "relar_sys_argv"
                | "relar_path_read_text"
                | "relar_path_write_text"
                | "relar_path_mkdir"
                | "relar_path_unlink"
                | "relar_path_rmdir"
                | "relar_path_replace"
                | "relar_path_resolve"
                | "relar_tempdir_new"
                | "relar_tempdir_cleanup"
                | "relar_path_next"
                | "relar_text_open"
                | "relar_text_enter"
                | "relar_text_read"
                | "relar_text_readline"
                | "relar_text_write"
                | "relar_text_flush"
                | "relar_text_close"
                | "relar_int_parse"
                | "relar_int_float"
                | "relar_float_int"
                | "relar_float_parse"
                | "relar_bytes_float"
                | "relar_int_true_divide"
                | "relar_assert_fail"
                | "relar_range_step_zero"
                | "relar_list_get"
                | "relar_list_set"
                | "relar_list_get_index"
                | "relar_list_set_index"
                | "relar_list_borrow_index"
                | "relar_list_copy_index"
                | "relar_list_pop"
                | "relar_list_repeat"
                | "relar_list_slice"
                | "relar_list_slice_sum"
                | "relar_str_get"
                | "relar_str_slice"
                | "relar_str_int_list"
                | "relar_seq_get"
                | "relar_seq_set"
                | "relar_seq_get_index"
                | "relar_seq_set_index"
                | "relar_seq_pop"
                | "relar_seq_repeat"
                | "relar_seq_slice"
                | "relar_dict_get"
                | "relar_dict_gather_str_int"
                | "relar_dict_gather_bytes_int"
                | "relar_dict_delete"
                | "relar_mapping_iter_next"
                | "relar_exception_raise"
                | "relar_exception_reraise"
        )
}

const DECLARATIONS: &str = r#"
declare ptr @relar_subprocess_run(ptr, ptr, ptr, i8, ptr)
declare ptr @relar_subprocess_run_timeout(ptr, ptr, ptr, i8, ptr, ptr)
declare ptr @relar_environ_copy()
declare void @relar_subprocess_check(ptr)
declare i64 @relar_subprocess_error_field(ptr, i32)
declare i64 @relar_subprocess_timeout_field(ptr, i32)
declare ptr @relar_path_new(ptr)
declare ptr @relar_path_join(ptr, ptr)
declare ptr @relar_path_text(ptr)
declare ptr @relar_path_name(ptr)
declare ptr @relar_path_stem(ptr)
declare ptr @relar_path_suffix(ptr)
declare ptr @relar_path_parent(ptr)
declare i8 @relar_path_is_absolute(ptr)
declare ptr @relar_path_resolve(ptr, i8)
declare i8 @relar_path_exists(ptr)
declare i8 @relar_path_is_file(ptr)
declare i8 @relar_path_is_dir(ptr)
declare i8 @relar_path_equal(ptr, ptr)
declare ptr @relar_path_read_text(ptr)
declare i64 @relar_path_write_text(ptr, ptr)
declare void @relar_path_mkdir(ptr, i64, i8, i8)
declare void @relar_path_unlink(ptr, i8)
declare void @relar_path_rmdir(ptr)
declare ptr @relar_path_replace(ptr, ptr)
declare ptr @relar_tempdir_new(ptr, ptr, ptr)
declare void @relar_tempdir_cleanup(ptr)
declare ptr @relar_path_iter(ptr, ptr)
declare ptr @relar_path_next(ptr)
declare i64 @relar_float_box(double)
declare double @relar_float_unbox(i64)
declare void @relar_float_box_retain(i64)
declare void @relar_float_box_release(i64)
declare ptr @relar_float_str(double)
declare void @relar_print_float(double)
declare double @relar_int_float(i64)
declare i64 @relar_float_int(double)
declare double @relar_float_parse(ptr)
declare double @relar_bytes_float(ptr)
declare double @relar_float_divide(double, double, i32)
declare i32 @relar_int_float_compare(i64, double)
declare double @relar_int_true_divide(i64, i64)
declare ptr @relar_bytes_literal(ptr, i64)
declare void @relar_bytes_retain(ptr)
declare void @relar_bytes_release(ptr)
declare ptr @relar_bytes_str(ptr)
declare void @relar_print_bytes(ptr)
declare i64 @relar_bytes_len(ptr)
declare i32 @relar_bytes_compare(ptr, ptr)
declare i64 @relar_bytes_get(ptr, i64)
declare i64 @relar_bytes_get_index(ptr, i64)
declare ptr @relar_bytes_slice(ptr, i64, i64, i64)
declare ptr @relar_bytes_count(i64)
declare ptr @relar_bytes_list(ptr)
declare ptr @relar_bytes_concat(ptr, ptr)
declare ptr @relar_bytes_repeat(ptr, i64)
declare ptr @relar_bytes_iter(ptr)
declare ptr @relar_str_encode(ptr)
declare ptr @relar_bytes_decode(ptr)
declare ptr @relar_bytes_validate_utf8(ptr)
declare ptr @relar_binary_open(ptr, i32)
declare ptr @relar_binary_read(ptr)
declare ptr @relar_binary_read_size(ptr, i64)
declare ptr @relar_binary_readline(ptr)
declare ptr @relar_gzip_open(ptr, i32)
declare i64 @relar_binary_write(ptr, ptr)

declare i8 @relar_error_pending()
declare ptr @relar_error_take()
declare void @relar_error_set(ptr)
declare i32 @relar_error_unhandled()
declare ptr @relar_exception_new(i32, ptr)
declare i8 @relar_exception_matches(ptr, i32)
declare void @relar_union_retain(i64)
declare void @relar_union_release(i64)
declare i8 @relar_union_truthy(i64)
declare ptr @relar_exception_message(ptr)
declare ptr @relar_exception_with_cause(ptr, ptr)
declare ptr @relar_exception_cause(ptr)
declare i8 @relar_exception_suppress_context(ptr)
declare ptr @relar_exception_current()
declare ptr @relar_exception_swap_active(ptr)
declare void @relar_exception_raise(ptr)
declare void @relar_exception_reraise()
declare void @relar_flush_stdout()
declare ptr @relar_record_new(ptr)
declare ptr @relar_json_decode(ptr, ptr)
declare ptr @relar_toml_decode(ptr, ptr)
declare ptr @relar_json_encode(i64, ptr, i64, i8)
declare ptr @relar_record_cached_result(ptr)
declare void @relar_record_retain(ptr)
declare void @relar_record_release(ptr)
declare i8 @relar_record_equal(ptr, ptr) nounwind memory(read)
declare i32 @relar_tuple_compare(ptr, ptr) nounwind memory(read)
declare ptr @relar_value_repr(i64, i32)
declare void @relar_object_retain(ptr)
declare void @relar_object_release(ptr)
declare ptr @relar_seq_new(i32)
declare i64 @relar_seq_get(ptr, i64)
declare void @relar_seq_set(ptr, i64, i64, i32)
declare i64 @relar_seq_get_index(ptr, i64)
declare i64 @relar_seq_borrow_index_proven(ptr, i64) nounwind memory(argmem: read)
declare ptr @relar_seq_data_readonly(ptr) nounwind memory(argmem: read)
declare void @relar_seq_set_index(ptr, i64, i64, i32)
declare void @relar_seq_append(ptr, i64, i32)
declare i64 @relar_seq_pop(ptr)
declare i64 @relar_seq_len(ptr) nounwind memory(argmem: read)
declare { i64, i64 } @relar_seq_layout() nounwind willreturn memory(none)
@sequence_layout = internal global { i64, i64 } zeroinitializer
define internal i32 @sequence_header(ptr %sequence) alwaysinline nounwind {
  %offset = load i64, ptr @sequence_layout
  %field = getelementptr inbounds i8, ptr %sequence, i64 %offset
  %length = load i32, ptr %field, align 4
  ret i32 %length
}
define internal i64 @relar_seq_len_fast(ptr %sequence) alwaysinline nounwind {
  %length = call i32 @sequence_header(ptr %sequence)
  %short = icmp ult i32 %length, -2
  br i1 %short, label %cached, label %large, !prof !1
cached:
  %size = zext i32 %length to i64
  %tagged = or i64 %size, 1
  ret i64 %tagged
large:
  %full = call i64 @relar_seq_len(ptr %sequence)
  ret i64 %full
}
define internal ptr @relar_seq_data_readonly_fast(ptr %sequence) alwaysinline nounwind {
  %header = call i32 @sequence_header(ptr %sequence)
  %inline = trunc i32 %header to i1
  br i1 %inline, label %small, label %spilled
small:
  %offset = load i64, ptr getelementptr ({ i64, i64 }, ptr @sequence_layout, i32 0, i32 1)
  %data = getelementptr inbounds i8, ptr %sequence, i64 %offset
  ret ptr %data
spilled:
  %heap = call ptr @relar_seq_data_readonly(ptr %sequence)
  ret ptr %heap
}
define internal i64 @relar_seq_borrow_index_proven_fast(ptr %sequence, i64 %index) alwaysinline nounwind {
  %data = call ptr @relar_seq_data_readonly_fast(ptr %sequence)
  %slot = getelementptr inbounds i64, ptr %data, i64 %index
  %word = load i64, ptr %slot
  ret i64 %word
}
declare ptr @relar_seq_repeat(ptr, i64)
declare ptr @relar_dict_new(i32, i32)
declare i64 @relar_dict_len(ptr)
declare i8 @relar_dict_contains(ptr, i64)
declare i64 @relar_dict_get(ptr, i64)
declare ptr @relar_dict_gather_str_int(ptr, ptr)
declare ptr @relar_dict_gather_bytes_int(ptr, ptr)
declare i8 @relar_dict_try_get(ptr, i64, ptr)
declare i64 @relar_dict_get_default(ptr, i64, i64)
declare void @relar_dict_set(ptr, i64, i64)
declare void @relar_dict_delete(ptr, i64)
declare ptr @relar_dict_iter(ptr, i32)
declare ptr @relar_set_new(i32)
declare i64 @relar_set_len(ptr)
declare i8 @relar_set_contains(ptr, i64)
declare void @relar_set_add(ptr, i64)
declare void @relar_set_discard(ptr, i64)
declare ptr @relar_set_iter(ptr)
declare i8 @relar_mapping_iter_next(ptr, ptr, ptr)
declare ptr @relar_str_concat(ptr, ptr)
declare ptr @relar_str_concat_parts(ptr, i64)
declare ptr @relar_str_concat_parts_owned(ptr, ptr, i64)
declare ptr @relar_str_append_utf8_owned(ptr, ptr, i64)
declare i8 @relar_str_get_part(ptr, i64, ptr, ptr)
declare ptr @relar_int_str_part(i64, ptr, ptr)
declare ptr @relar_str_split_whitespace(ptr)
declare ptr @relar_str_split(ptr, ptr, i64, i8)
declare ptr @relar_str_rsplit_last(ptr, ptr)
declare ptr @relar_str_unquote_rsplit_last(ptr, ptr)
declare ptr @relar_str_strip(ptr, ptr, i32)
declare i8 @relar_str_affix(ptr, ptr, i8)
declare i64 @relar_str_find(ptr, ptr, i64)
declare void @relar_str_partition(ptr, ptr, ptr)
declare ptr @relar_str_unquote(ptr)
declare ptr @relar_str_splitlines(ptr)
declare ptr @relar_str_rstrip(ptr, ptr)
declare void @relar_sort_by_keys(ptr, i32, ptr, i32, i8)
declare ptr @relar_str_join(ptr, ptr)
declare ptr @relar_str_replace(ptr, ptr, ptr)
declare ptr @relar_float_format(double, i32, i8)
declare double @llvm.fabs.f64(double)
declare ptr @relar_int_str(i64)
declare ptr @relar_bool_str(i8)
declare ptr @relar_none_str()
declare ptr @relar_text_open(ptr, i32)
declare i8 @relar_text_enter(ptr)
declare ptr @relar_text_read(ptr)
declare ptr @relar_text_readline(ptr)
declare i64 @relar_text_write(ptr, ptr)
declare i8 @relar_text_write_discard(ptr, ptr)
declare i8 @relar_text_write_parts(ptr, ptr, i64)
declare i8 @relar_text_flush(ptr)
declare i8 @relar_text_close(ptr)
declare i8 @relar_text_closed(ptr)
declare void @relar_args_init(i32, ptr)
declare ptr @relar_sys_argv()
declare void @relar_args_clear()
declare void @relar_int_retain(i64)
declare void @relar_int_release(i64)
declare i64 @relar_int_literal(ptr, i64)
declare i64 @relar_int_binary(i64, i64, i32)
declare i32 @relar_int_compare(i64, i64)
declare void @relar_print_int(i64)
declare void @relar_print_bool(i8)
declare void @relar_print_none()
declare ptr @relar_str_literal(ptr, i64)
declare ptr @relar_regex_compile(ptr, i64)
declare ptr @relar_regex_search(ptr, ptr)
declare ptr @relar_regex_group(ptr, i64, i8)
declare i8 @relar_regex_present(ptr, i64, i8)
declare i64 @relar_regex_bound(ptr, i64, i8, i8)
declare ptr @relar_str_get(ptr, i64)
declare ptr @relar_str_slice(ptr, i64, i64, i64)
declare ptr @relar_str_iter(ptr)
declare ptr @relar_str_reverse_iter(ptr)
declare ptr @relar_str_int_list(ptr)
declare void @relar_str_retain(ptr)
declare void @relar_str_release(ptr)
declare void @relar_print_str(ptr)
declare i64 @relar_str_len(ptr)
declare i8 @relar_str_truthy(ptr)
declare i8 @relar_bytes_truthy(ptr)
declare i8 @relar_str_list_contains(ptr, ptr)
declare void @relar_str_enum_table(ptr, i64)
declare i8 @relar_str_equal(ptr, ptr)
declare i64 @relar_int_parse(ptr)
declare ptr @relar_input()
declare ptr @relar_list_new()
declare i64 @relar_list_sum(ptr, i64)
declare i64 @relar_list_slice_sum(ptr, i64, i64, i64)
declare i8 @relar_list_all_i32(ptr, i64, i64, i64)
declare void @relar_list_reserve_range(ptr, i64, i64, i64, i64)
declare void @relar_list_retain(ptr)
declare void @relar_list_release(ptr)
declare i64 @relar_list_get(ptr, i64)
declare i8 @relar_list_set(ptr, i64, i64)
declare i64 @relar_list_get_index(ptr, i64)
declare i8 @relar_list_set_index(ptr, i64, i64)
declare i64 @relar_list_borrow_index(ptr, i64)
declare i64 @relar_list_borrow_index_proven(ptr, i64) nounwind memory(argmem: read)
declare i8 @relar_list_copy_index(ptr, i64, i64)
declare void @relar_list_append(ptr, i64)
declare void @relar_list_append_pair(ptr, i64, i64)
declare i64 @relar_list_pop(ptr)
declare i64 @relar_list_len(ptr)
declare ptr @relar_list_repeat(ptr, i64)
declare ptr @relar_list_slice(ptr, i64, i64, i64)
declare ptr @relar_seq_slice(ptr, i64, i64, i64)
declare void @relar_assert_fail()
declare void @relar_range_step_zero()
declare void @llvm.assume(i1)
declare { i64, i1 } @llvm.sadd.with.overflow.i64(i64, i64)
declare { i64, i1 } @llvm.ssub.with.overflow.i64(i64, i64)
declare { i64, i1 } @llvm.smul.with.overflow.i64(i64, i64)
"#;

/// Expose zero-owner cleanup to LLVM even when the runtime is linked without LTO.
/// After mem2reg, a cleared or never-filled slot needs no runtime call at all.
fn nullable_pointer_helpers() -> String {
    let mut output = String::new();
    for family in ["str", "object", "record"] {
        for operation in ["release", "retain"] {
            writeln!(
                output,
                r#"
define internal void @{family}_{operation}_if_present(ptr %value) alwaysinline {{
entry:
  %absent = icmp eq ptr %value, null
  br i1 %absent, label %done, label %present
present:
  call void @relar_{family}_{operation}(ptr %value)
  br label %done
done:
  ret void
}}"#
            )
            .unwrap();
        }
    }
    output
}

/// Operates on tagged integers directly. For E(x) = 2x + 1, addition and
/// subtraction produce E(x +/- y); multiplication first produces 2xy.
/// Signed overflow is exactly failure to fit the tagged-small range.
/// Explicit odd results expose representation facts after helper inlining;
/// cold ownership boundaries keep heap destructors out of immediate fast paths.
fn integer_helpers() -> String {
    let mut output = String::new();
    for operation in ["retain", "release"] {
        writeln!(
            output,
            r#"
define internal void @{operation}_int_heap(i64 %value) cold noinline {{
entry:
  call void @relar_int_{operation}(i64 %value)
  ret void
}}"#
        )
        .unwrap();
        for nonzero in [false, true] {
            let (suffix, presence, immediate) = if nonzero {
                ("_nonzero", "", "%small")
            } else {
                (
                    "",
                    "  %absent = icmp eq i64 %value, 0\n  %immediate = or i1 %small, %absent\n",
                    "%immediate",
                )
            };
            writeln!(
                output,
                r#"
define internal void @{operation}_int{suffix}(i64 %value) alwaysinline {{
entry:
  %tag = and i64 %value, 1
  %small = icmp eq i64 %tag, 1
{presence}  br i1 {immediate}, label %done, label %big, !prof !1
big:
  call void @{operation}_int_heap(i64 %value)
  br label %done
done:
  ret void
}}"#
            )
            .unwrap();
        }
    }
    for (op, intrinsic, number) in [("add", "sadd", 0), ("sub", "ssub", 1), ("mul", "smul", 2)] {
        let (left, decode) = if op == "mul" {
            ("%x", "  %x = ashr i64 %a, 1\n")
        } else {
            ("%a", "")
        };
        writeln!(
            output,
            r#"
define internal i64 @int_{op}(i64 %a, i64 %b) alwaysinline {{
entry:
  %both = and i64 %a, %b
  %tag = and i64 %both, 1
  %small = icmp eq i64 %tag, 1
  br i1 %small, label %fast, label %big, !prof !1
fast:
{decode}  %right = sub i64 %b, 1
  %pair = call {{ i64, i1 }} @llvm.{intrinsic}.with.overflow.i64(i64 {left}, i64 %right)
  %result = extractvalue {{ i64, i1 }} %pair, 0
  %overflow = extractvalue {{ i64, i1 }} %pair, 1
  br i1 %overflow, label %big, label %done, !prof !0
done:
  %encoded = or i64 %result, 1
  ret i64 %encoded
big:
  %slow = call i64 @relar_int_binary(i64 %a, i64 %b, i32 {number}) cold
  ret i64 %slow
}}"#
        )
        .unwrap();
    }
    output.push_str(conditionals::INTEGER_ABSOLUTE_HELPER);
    output.push_str(&division_helpers());
    output.push_str(&bitwise_helpers());
    output.push_str(
        r#"
define internal i32 @compare_int(i64 %a, i64 %b) alwaysinline {
entry:
  %both = and i64 %a, %b
  %tag = and i64 %both, 1
  %small = icmp eq i64 %tag, 1
  br i1 %small, label %fast, label %big, !prof !1
fast:
  %less = icmp slt i64 %a, %b
  %equal = icmp eq i64 %a, %b
  %nonless = select i1 %equal, i32 0, i32 1
  %order = select i1 %less, i32 -1, i32 %nonless
  ret i32 %order
big:
  %slow = call i32 @relar_int_compare(i64 %a, i64 %b) cold
  ret i32 %slow
}
"#,
    );
    output
}

/// Decoded small integers cannot overflow i64 division. Guard zero before any
/// division, apply Python's floor/sign rule, and promote MIN_SMALL / -1 rather
/// than wrapping its result back into a tagged integer.
fn division_helpers() -> String {
    let mut output = String::new();
    for (op, number, calculation) in [
        (
            "floordiv",
            3,
            r#"  %quotient = sdiv i64 %x, %y
  %adjustment = zext i1 %adjust to i64
  %result = sub i64 %quotient, %adjustment
  %over = icmp sgt i64 %result, 4611686018427387903
  br i1 %over, label %slow, label %done"#,
        ),
        (
            "mod",
            4,
            r#"  %adjustment = select i1 %adjust, i64 %y, i64 0
  %result = add i64 %remainder, %adjustment
  br label %done"#,
        ),
    ] {
        writeln!(
            output,
            r#"
define internal i64 @int_{op}(i64 %a, i64 %b) alwaysinline {{
entry:
  %both = and i64 %a, %b
  %tag = and i64 %both, 1
  %small = icmp eq i64 %tag, 1
  %nonzero_divisor = icmp ne i64 %b, 1
  %can_fast = and i1 %small, %nonzero_divisor
  br i1 %can_fast, label %fast, label %slow
fast:
  %x = ashr i64 %a, 1
  %y = ashr i64 %b, 1
  %remainder = srem i64 %x, %y
  %nonzero_remainder = icmp ne i64 %remainder, 0
  %signs = xor i64 %remainder, %y
  %opposite = icmp slt i64 %signs, 0
  %adjust = and i1 %nonzero_remainder, %opposite
{calculation}
done:
  %shift = shl i64 %result, 1
  %encoded = or i64 %shift, 1
  ret i64 %encoded
slow:
  %fallback = call i64 @relar_int_binary(i64 %a, i64 %b, i32 {number})
  ret i64 %fallback
}}"#
        )
        .unwrap();
    }
    output
}

/// Tagged bitwise results remain small. Shifts guard LLVM's count and signed
/// overflow constraints before falling back to arbitrary-precision arithmetic.
fn bitwise_helpers() -> String {
    let mut output = String::new();
    for (op, number) in [("and", 5), ("or", 6), ("xor", 7)] {
        writeln!(
            output,
            r#"
define internal i64 @int_{op}(i64 %a, i64 %b) alwaysinline {{
entry:
  %both = and i64 %a, %b
  %tag = and i64 %both, 1
  %small = icmp eq i64 %tag, 1
  br i1 %small, label %fast, label %slow
fast:
  %result = {op} i64 %a, %b
  %encoded = or i64 %result, 1
  ret i64 %encoded
slow:
  %fallback = call i64 @relar_int_binary(i64 %a, i64 %b, i32 {number})
  ret i64 %fallback
}}"#
        )
        .unwrap();
    }
    for (op, number, calculation) in [
        (
            "lshift",
            8,
            r#"  %safe_count = icmp ult i64 %y, 63
  br i1 %safe_count, label %shift, label %slow
shift:
  %result = shl i64 %x, %y
  %restored = ashr i64 %result, %y
  %lost = icmp ne i64 %restored, %x
  %under = icmp slt i64 %result, -4611686018427387904
  %over = icmp sgt i64 %result, 4611686018427387903
  %outside = or i1 %under, %over
  %overflow = or i1 %lost, %outside
  br i1 %overflow, label %slow, label %done"#,
        ),
        (
            "rshift",
            9,
            r#"  %within = icmp ult i64 %y, 63
  %count = select i1 %within, i64 %y, i64 63
  %result = ashr i64 %x, %count
  br label %done"#,
        ),
    ] {
        writeln!(
            output,
            r#"
define internal i64 @int_{op}(i64 %a, i64 %b) alwaysinline {{
entry:
  %both = and i64 %a, %b
  %tag = and i64 %both, 1
  %small = icmp eq i64 %tag, 1
  %nonnegative = icmp sge i64 %b, 1
  %can_fast = and i1 %small, %nonnegative
  br i1 %can_fast, label %fast, label %slow
fast:
  %x = ashr i64 %a, 1
  %y = ashr i64 %b, 1
{calculation}
done:
  %shifted = shl i64 %result, 1
  %encoded = or i64 %shifted, 1
  ret i64 %encoded
slow:
  %fallback = call i64 @relar_int_binary(i64 %a, i64 %b, i32 {number})
  ret i64 %fallback
}}"#
        )
        .unwrap();
    }
    output
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn fixed_patterns_compile_once_across_search_sites() {
        let llvm = compile(
            "import re\n\ndef first(text: str) -> int:\n    if m := re.search('a+', text):\n        return m.start()\n    return -1\nprint(first('aa'))\nprint(bool(re.compile('a+').search('b')))\n",
            &CompileOptions::default(),
        )
        .unwrap();
        assert_eq!(llvm.matches("call ptr @relar_regex_compile(").count(), 1);
        assert_eq!(llvm.matches("call ptr @relar_regex_search(").count(), 2);
        let cleanup = llvm
            .split_once("define internal void @relar_clear_literals(")
            .unwrap()
            .1;
        assert!(
            cleanup
                .split_once("\n}")
                .unwrap()
                .0
                .contains("call void @relar_object_release(")
        );
    }

    #[test]
    fn string_constants_are_rooted_once_and_cleared_on_both_main_exits() {
        let llvm = compile(
            "def text() -> str:\n    return 'shared'\nprint('shared')\nprint(text())\n",
            &CompileOptions {
                address_sanitizer: true,
            },
        )
        .unwrap();
        assert_eq!(llvm.matches("call ptr @relar_str_literal(").count(), 1);
        for operation in ["init", "clear"] {
            assert!(llvm.contains(&format!(
                "define internal void @relar_{operation}_literals() sanitize_address {{"
            )));
        }
        let body = llvm
            .split_once("define internal ptr @fn_0(")
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0;
        assert!(!body.contains("@relar_str_literal("));
        assert!(body.contains("@relar_str_retain("));
        let main = llvm.split_once("define i32 @main(").unwrap().1;
        assert_eq!(main.matches("call void @relar_init_literals()").count(), 1);
        assert_eq!(main.matches("call void @relar_clear_literals()").count(), 2);
    }

    /// Classifies only assignments to a source slot, excluding its argument and
    /// zero-initialization stores. A generic replacement releases its old load
    /// immediately after publishing the new value.
    fn update_releases(body: &str, slot: usize) -> Vec<bool> {
        let lines = body.lines().collect::<Vec<_>>();
        lines
            .windows(2)
            .filter(|lines| {
                lines[0].trim_start().starts_with("store i64 %v")
                    && lines[0].ends_with(&format!(", ptr %local{slot}"))
            })
            .map(|lines| lines[1].contains("call void @release_int"))
            .collect()
    }

    /// Isolates source-function instructions from the always-emitted fallbacks.
    fn function_body(source: &str) -> String {
        let llvm = compile(source, &CompileOptions::default()).unwrap();
        llvm.split_once("@fn_0(")
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0
            .to_owned()
    }

    #[test]
    fn exact_integer_owners_keep_nullable_record_and_optional_cleanup_separate() {
        let exact =
            function_body("def keep(value: int) -> int:\n    value = value\n    return value\n");
        assert!(exact.contains("call void @retain_int_nonzero("));
        assert!(exact.contains("call void @release_int_nonzero("));
        assert!(!exact.contains("call void @release_int("));

        let optional = function_body(
            "def keep(value: int | None) -> int | None:\n    value = value\n    return value\n",
        );
        assert!(optional.contains("call void @retain_int("));
        assert!(optional.contains("call void @release_int("));
        assert!(!optional.contains("call void @release_int_nonzero("));

        let initializer = function_body(
            "class Box:\n    value: int\n    def __init__(self, value: int) -> None:\n        self.value = value\n",
        );
        assert!(initializer.contains("call void @retain_int_nonzero("));
        assert!(initializer.contains("call void @release_int("));

        let helpers = super::integer_helpers();
        for operation in ["retain", "release"] {
            let body = helpers
                .split_once(&format!("@{operation}_int_nonzero("))
                .unwrap()
                .1
                .split_once("\n}")
                .unwrap()
                .0;
            assert!(!body.contains("%absent"));
            assert!(body.contains("br i1 %small"));
            assert!(body.contains(&format!("call void @{operation}_int_heap(i64 %value)")));
            assert!(helpers.contains(&format!(
                "define internal void @{operation}_int_heap(i64 %value) cold noinline"
            )));
        }
    }

    #[test]
    fn integer_fast_results_are_odd_only_after_the_overflow_guard() {
        let helpers = super::integer_helpers();
        for (operation, number) in [("add", 0), ("sub", 1), ("mul", 2)] {
            let body = helpers
                .split_once(&format!("define internal i64 @int_{operation}("))
                .unwrap()
                .1
                .split_once("\n}")
                .unwrap()
                .0;
            let guard = body
                .find("br i1 %overflow, label %big, label %done")
                .unwrap();
            let encoded = body.find("%encoded = or i64 %result, 1").unwrap();
            assert!(guard < encoded, "{body}");
            assert!(body.contains("br i1 %small, label %fast, label %big, !prof !1"));
            assert!(body.contains("br i1 %overflow, label %big, label %done, !prof !0"));
            let (_, slow) = body.split_once("big:").unwrap();
            assert!(slow.contains(&format!(
                "call i64 @relar_int_binary(i64 %a, i64 %b, i32 {number}) cold"
            )));
            assert!(slow.contains("ret i64 %slow"));
            assert!(
                !slow.contains("or i64"),
                "heap/cancelled results must remain unchanged"
            );
        }
    }

    #[test]
    fn suspended_list_pipeline_cursors_use_native_arithmetic() {
        for (source, operation, fallback) in [
            ("values", "add nsw i64", "@int_add("),
            ("reversed(values)", "sub nsw i64", "@int_sub("),
        ] {
            let body = function_body(&format!(
                "def same(values: list[int]) -> bool:\n    return all(left == right for left, right in zip({source}, {source}))\n"
            ));
            assert!(body.contains(operation));
            assert!(!body.contains(fallback));
            assert!(body.contains("@relar_list_get_index("));
            assert!(!body.contains("@relar_list_get("));
        }
    }

    #[test]
    fn proven_sequence_reads_acquire_the_static_element_without_an_error_poll() {
        for (ty, retain) in [
            ("str", Some("@relar_str_retain(")),
            ("str | None", Some("@str_retain_if_present(")),
            ("int | None", Some("@retain_int(")),
            ("float | None", Some("@relar_float_box_retain(")),
            ("list[int]", Some("@relar_list_retain(")),
            ("tuple[int]", Some("@relar_record_retain(")),
            ("bool | None", None),
            ("float", None),
            ("None", None),
        ] {
            // Rebinding excludes a stable caller root, so this exercises the
            // independently owned pull rather than projection borrowing.
            let body = function_body(&format!(
                "def populated(values: list[{ty}]) -> bool:\n    values = values\n    return any(value for value in values)\n"
            ));
            let (_, pull) = body
                .split_once("@relar_seq_borrow_index_proven_fast(")
                .unwrap();
            if let Some(retain) = retain {
                let acquired = pull.split_once("store ").unwrap().0;
                assert!(acquired.contains(retain), "{ty}: {acquired}");
            }
            assert!(!body.contains("@relar_seq_get("));
            assert!(!body.contains("@relar_seq_get_index("));
            assert!(!body.contains("@relar_error_pending("));
        }
        let checked = function_body(
            "def item(values: list[str], index: int) -> str:\n    return values[index]\n",
        );
        assert!(checked.contains("@relar_seq_get("));
        assert!(checked.contains("@relar_error_pending("));
    }

    #[test]
    fn bounded_accumulator_identity_keeps_native_and_fallback_arithmetic_without_extra_uses() {
        for term in ["10", "-10"] {
            let body = function_body(&format!(
                "def total(values: list[int]) -> int:\n    result = 0\n    cursor = 0\n    while cursor < len(values):\n        cursor += 1\n        result += {term}\n    return result\n"
            ));
            assert!(
                !body.contains("call void @llvm.assume"),
                "the accumulator proof must not add a second use of its recurrence"
            );
            assert!(body.contains("add nsw i64"));
            assert!(
                body.contains("call i64 @int_add"),
                "lists outside the checked subtotal budget retain arbitrary-precision arithmetic"
            );
            assert!(body.contains("icmp sle i64"));
        }
    }

    #[test]
    fn bounded_reduction_proofs_do_not_request_forced_vectorization() {
        let llvm = compile(
            r#"
def total(text: str) -> int:
    digits = list(map(int, text))
    result = 0
    cursor = 0
    while cursor < len(digits):
        digit = digits[cursor]
        cursor += 1
        if digit == 0:
            continue
        result += digit
    return result
"#,
            &CompileOptions::default(),
        )
        .unwrap();
        assert!(llvm.contains("call i64 @relar_list_borrow_index_proven("));
        assert!(llvm.contains("add nsw i64"));
        assert!(llvm.contains("call i64 @int_add"));
        assert!(
            !llvm.contains("!llvm.loop"),
            "bounded arithmetic and access proofs do not guarantee backend vectorization"
        );
        assert!(!llvm.contains("llvm.loop.vectorize"));
    }

    #[test]
    fn adjacent_integer_local_appends_share_one_capacity_check() {
        for (source, singles) in [
            (
                "def push(output: list[int], first: int, second: int) -> None:\n    output.append(first)\n    output.append(second)\n",
                0,
            ),
            (
                "def push(output: list[int], first: int, second: int) -> None:\n    output.append(first)\n    output.append(second)\n    output.append(first)\n",
                1,
            ),
            (
                "def push(output: list[int], value: int) -> None:\n    output.extend(divmod(value, 256))\n",
                0,
            ),
            (
                "def push(output: list[int], first: int, second: int) -> None:\n    if (first := second) > 0:\n        pass\n    output.append(first)\n    output.append(second)\n",
                0,
            ),
        ] {
            let body = function_body(source);
            assert_eq!(
                body.matches("call void @relar_list_append_pair(").count(),
                1
            );
            assert_eq!(
                body.matches("call void @relar_list_append(").count(),
                singles
            );
        }
    }

    #[test]
    fn append_pair_requires_adjacent_pure_locals_of_the_same_integer_list() {
        for statements in [
            "    output.append(first)\n    other.append(second)\n",
            "    output.append(first)\n    output.append(1)\n",
            "    output.append(1)\n    output.append(second)\n",
            "    output.append(first)\n    output.append(len(output))\n",
            "    output.append(len(output))\n    output.append(second)\n",
            "    output.append(first)\n    second = first\n    output.append(second)\n",
        ] {
            let body = function_body(&format!(
                "def push(output: list[int], other: list[int], first: int, second: int) -> None:\n{statements}"
            ));
            assert!(!body.contains("call void @relar_list_append_pair("));
            assert_eq!(body.matches("call void @relar_list_append(").count(), 2);
        }
        let body = function_body(
            "def push(output: list[str], first: str, second: str) -> None:\n    output.append(first)\n    output.append(second)\n",
        );
        assert!(!body.contains("call void @relar_list_append_pair("));
        assert_eq!(body.matches("call void @relar_seq_append(").count(), 2);
        let body = function_body(
            "def push(outputs: list[list[int]], first: int, second: int) -> None:\n    outputs[0].append(first)\n    outputs[0].append(second)\n",
        );
        assert!(!body.contains("call void @relar_list_append_pair("));
        assert_eq!(body.matches("call void @relar_list_append(").count(), 2);
    }

    #[test]
    fn native_power_of_two_division_uses_python_signed_shift_and_mask() {
        for divisor in [1_u64, 2, 256, 1 << 61] {
            for operation in ["//", "%"] {
                let body = function_body(&format!(
                    "def calculate(value: int) -> int:\n    if -1024 <= value <= 1024:\n        divisor = {divisor}\n        return value {operation} divisor\n    return 0\n"
                ));
                let (instruction, constant) = if operation == "//" {
                    ("ashr", u64::from(divisor.trailing_zeros()))
                } else {
                    ("and", divisor - 1)
                };
                assert!(body.lines().any(|line| {
                    line.contains(&format!("= {instruction} i64 "))
                        && line.ends_with(&format!(", {constant}"))
                }));
                assert!(!body.contains("sdiv i64"));
                assert!(!body.contains("srem i64"));
                assert!(!body.contains("call i64 @int_floordiv"));
                assert!(!body.contains("call i64 @int_mod"));
            }
        }
    }

    #[test]
    fn native_division_keeps_fallbacks_without_a_positive_power_of_two_proof() {
        for operation in ["//", "%"] {
            for divisor in ["-256", "3"] {
                let body = function_body(&format!(
                    "def calculate(value: int) -> int:\n    if -1024 <= value <= 1024:\n        return value {operation} {divisor}\n    return 0\n"
                ));
                let instruction = if operation == "//" { "sdiv" } else { "srem" };
                assert!(body.contains(&format!("{instruction} i64")));
            }
            let helper = if operation == "//" { "floordiv" } else { "mod" };
            for divisor in ["0", "256", "18446744073709551616"] {
                let body = function_body(&format!(
                    "def calculate(value: int) -> int:\n    return value {operation} {divisor}\n"
                ));
                assert!(body.contains(&format!("call i64 @int_{helper}")));
            }
            let body = function_body(&format!(
                "def calculate(value: int, divisor: int) -> int:\n    if -1024 <= value <= 1024 and 1 <= divisor <= 4:\n        return value {operation} divisor\n    return 0\n"
            ));
            // Interval division currently requires a singleton divisor. A
            // merely bounded divisor therefore retains the complete helper.
            assert!(body.contains(&format!("call i64 @int_{helper}")));
        }
    }

    #[test]
    fn bounded_nonnegative_division_uses_a_single_comparison() {
        for divisor in [3_i64, 10, (1 << 61) + 1] {
            let upper = (2 * divisor - 1).min((1 << 62) - 1);
            for operation in ["//", "%"] {
                let body = function_body(&format!(
                    "def calculate(value: int) -> int:\n    if 0 <= value <= {upper}:\n        return value {operation} {divisor}\n    return 0\n"
                ));
                assert!(body.contains("icmp uge i64"));
                assert!(!body.contains("sdiv i64"));
                assert!(!body.contains("srem i64"));
                assert!(!body.contains("call i64 @int_floordiv"));
                assert!(!body.contains("call i64 @int_mod"));
            }
        }
    }

    #[test]
    fn comparison_division_requires_nonnegative_and_strict_upper_bounds() {
        for (lower, upper, divisor) in [(-1, 19, 10), (0, 20, 10), (0, 19, -10)] {
            for operation in ["//", "%"] {
                let body = function_body(&format!(
                    "def calculate(value: int) -> int:\n    if {lower} <= value <= {upper}:\n        return value {operation} {divisor}\n    return 0\n"
                ));
                let instruction = if operation == "//" { "sdiv" } else { "srem" };
                assert!(body.contains(&format!("{instruction} i64")));
            }
        }
    }

    #[test]
    fn assignment_cleanup_uses_the_same_slots_evaluated_small_value() {
        for right in ["increment", "sum(divmod(increment, 10))"] {
            let body = function_body(&format!(
                "def calculate(value: int, increment: int) -> int:\n    if 0 <= value <= 9:\n        value = value + {right}\n    return value\n"
            ));
            assert_eq!(update_releases(&body, 0), [false]);
        }
        let llvm = compile(
            "def total(text: str) -> int:\n    digits = list(map(int, text))\n    return sum([sum(divmod(2 * digit, 10)) for digit in digits])\n",
            &CompileOptions::default(),
        )
        .unwrap();
        let body = llvm
            .split_once("define internal i64 @fn_1(")
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0;
        // The private subtotal is local1: its guarded update is immediate, and
        // its arbitrary-precision fallback still releases the previous owner.
        assert_eq!(update_releases(body, 1), [false, true]);
    }

    #[test]
    fn assignment_cleanup_retains_unknown_and_expression_rebound_owners() {
        let body = function_body(
            "def calculate(value: int, increment: int) -> int:\n    value = value + increment\n    return value\n",
        );
        assert_eq!(update_releases(&body, 0), [true]);
        let body = function_body(
            "def calculate(value: int) -> int:\n    if 0 <= value <= 9:\n        value = value + (value := 100000000000000000000000000000000)\n    return value\n",
        );
        assert_eq!(update_releases(&body, 0), [true, true]);
    }

    #[test]
    fn assignment_cleanup_rejects_hidden_scope_replacement_after_the_read() {
        use crate::ir::{Expr, ExprKind, Statement, Type};

        let graph = crate::modules::SourceGraph::single(
            "def calculate(value: int, increment: int) -> int:\n    if 0 <= value <= 9:\n        value = value + increment\n    return value\n",
        )
        .unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        let Statement::If(_, body, _) = &mut module.functions[0].body[0] else {
            unreachable!()
        };
        let Statement::Assign(slot, expression) = &mut body[0] else {
            unreachable!()
        };
        let ExprKind::Binary(_, _, right) = &mut expression.kind else {
            unreachable!()
        };
        // Exercise a statement-owned write without a Bind node: the earlier
        // operand stays small, but the assignment must release this new BigInt.
        **right = Expr {
            ty: Type::Int,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(
                    *slot,
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Int("100000000000000000000000000000000".into()),
                    },
                )],
                value: right.clone(),
                clear: Vec::new(),
            },
        };
        assert!(!module.functions[0].expression_rebound()[0]);
        let ranges = crate::ranges::analyze(&module);
        let llvm = super::emit(&module, &ranges, false);
        let body = llvm
            .split_once("@fn_0(")
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0;
        assert_eq!(update_releases(body, 0), [true, true]);
    }
}
