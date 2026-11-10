//! Link checked module registrations to their native initializers.

use super::registered::{ABI, Output};
use crate::CodegenUnit;
use crate::ir::{ModuleLoader, ModuleRegistration, PhysicalModuleKind};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

const SOURCE: &str = "imre_dyn_register_source";
const BUILTIN: &str = "imre_dyn_register_builtin_source";
const FILE: &str = "imre_dyn_register_file_code";
const ZIP: &str = "imre_dyn_register_zip_source";
const ERROR: &str = "imre_dyn_register_module_error";

/// Root constants, registration calls and declarations for one output layout.
#[derive(Clone)]
pub(crate) struct ModuleLauncher {
    pub(crate) output: Output,
    main: String,
    declarations: HashSet<String>,
}

impl ModuleLauncher {
    pub(crate) fn new(
        config: &str,
        keep_docstrings: bool,
        has_zip: bool,
        has_builtins: bool,
        has_errors: bool,
    ) -> Self {
        let mut output = Output {
            keep_docstrings,
            ..Output::default()
        };
        let config = output.data(config.as_bytes());
        // Keep entry-only declarations out of standalone provider fingerprints.
        let source_args = "ptr, ptr, ptr, ptr, i64, ptr";
        let physical_args = "ptr, ptr, ptr, ptr, i32, ptr";
        let main = format!(
            "declare void @{SOURCE}({source_args})\n\ndefine i32 @main(i32 %argc, ptr %argv) {{\nentry:\n  %ctx = call ptr @imre_dyn_new(i32 %argc, ptr %argv, ptr {config})\n"
        );
        writeln!(output.functions, "declare void @{FILE}({physical_args})").unwrap();
        if has_zip {
            writeln!(output.functions, "declare void @{ZIP}({physical_args})").unwrap();
        }
        if has_builtins {
            writeln!(output.functions, "declare void @{BUILTIN}({source_args})").unwrap();
        }
        if has_errors {
            writeln!(
                output.functions,
                "declare void @{ERROR}(ptr, ptr, ptr, ptr, i64, ptr, ptr, i8, ptr)"
            )
            .unwrap();
        }
        Self {
            output,
            main,
            declarations: HashSet::new(),
        }
    }

    pub(crate) fn declares(&self, symbol: &str) -> bool {
        self.declarations.contains(symbol)
    }

    // The name and file precede every initializer's constants, including a stored error.
    fn module_operands(&mut self, name: &str, file: &Path) -> (String, String) {
        (
            self.output.data(name.as_bytes()),
            self.output.data(file.as_os_str().as_encoded_bytes()),
        )
    }

    pub(crate) fn module_registration<'a>(
        &mut self,
        registration: &'a ModuleRegistration<'_>,
    ) -> ModuleOperands<'a> {
        let (name, file) = self.module_operands(registration.name, registration.file);
        ModuleOperands {
            name,
            file,
            loader: &registration.loader,
        }
    }

    pub(crate) fn external(&mut self, symbol: &str) -> String {
        if self.declarations.insert(symbol.to_owned()) {
            writeln!(self.output.functions, "declare i64 @{symbol}(ptr, i64)").unwrap();
        }
        format!("@{symbol}")
    }

    pub(crate) fn register_error(
        &mut self,
        (name, file, source): (&str, &Path, &str),
        message: &str,
        position: Option<[usize; 4]>,
        in_args: bool,
        parse_error: bool,
        digest: Option<&[u8; 32]>,
    ) {
        let (name, file) = self.module_operands(name, file);
        let message = self.output.data(message.as_bytes());
        let position = position.map_or_else(|| "null".into(), |span| self.output.position(span));
        let text = self.output.data(source.as_bytes());
        let hash = digest.map_or_else(|| "null".into(), |digest| self.output.data(digest));
        let error_kind = if parse_error { 2 } else { u8::from(in_args) };
        writeln!(
            self.main,
            "  call void @{ERROR}(ptr %ctx, ptr {name}, ptr {file}, ptr {text}, i64 {}, ptr {message}, ptr {position}, i8 {error_kind}, ptr {hash})",
            source.len(),
        )
        .unwrap();
    }

    pub(crate) fn finish(mut self) -> CodegenUnit {
        self.output.integer_prelude();
        self.main
            .push_str("  %status = call i32 @imre_dyn_finish(ptr %ctx)\n  ret i32 %status\n}\n");
        CodegenUnit {
            name: "__main__".into(),
            llvm_ir: format!(
                "{ABI}\n{}\n{}\n{}",
                self.output.constants, self.output.functions, self.main
            ),
        }
    }
}

/// The loader payload follows the initializer's constants in each output.
pub(crate) struct ModuleOperands<'a> {
    name: String,
    pub(crate) file: String,
    loader: &'a ModuleLoader<'a>,
}

impl ModuleOperands<'_> {
    pub(crate) fn write_call(self, launcher: &mut ModuleLauncher, code: &str) {
        let name = self.name;
        let file = self.file;
        let (function, data, argument) = match self.loader {
            ModuleLoader::Source { builtin, text } => {
                let function = if *builtin { BUILTIN } else { SOURCE };
                let data = text.map_or_else(|| "null".into(), |text| launcher.output.data(text));
                (
                    function,
                    data,
                    format!("i64 {}", text.map_or(0, <[u8]>::len)),
                )
            }
            ModuleLoader::Physical {
                kind,
                digest,
                future_flags,
            } => {
                let function = match kind {
                    PhysicalModuleKind::File => FILE,
                    PhysicalModuleKind::Zip => ZIP,
                };
                (
                    function,
                    launcher.output.data(digest),
                    format!("i32 {future_flags}"),
                )
            }
        };
        writeln!(
            launcher.main,
            "  call void @{function}(ptr %ctx, ptr {name}, ptr {file}, ptr {data}, {argument}, ptr {code})"
        )
        .unwrap();
    }
}
