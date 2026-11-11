//! Link checked module registrations to their native initializers.

use super::body::FunctionBody;
use super::registered::{ABI, Output};
use crate::CodegenUnit;
use crate::ir::{ModuleLoader, ModuleRegistration, PhysicalModuleKind};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

const SOURCE: &str = "imre_dyn_register_source";
const BUILTIN: &str = "imre_dyn_register_builtin_source";
const FILE: &str = "imre_dyn_register_file_code";
const DECLARED_FILE: &str = "imre_dyn_register_declared_file_code_at";
const ZIP: &str = "imre_dyn_register_zip_source";
const ERROR: &str = "imre_dyn_register_module_error";
const SINGLE: &str = "imre_dyn_register_module_single";

/// Root constants, registration calls and declarations for one output layout.
#[derive(Clone)]
pub(crate) struct ModuleLauncher {
    pub(crate) output: Output,
    main: FunctionBody,
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
        let mut main = FunctionBody::new("entry", "");
        writeln!(main.code, "declare void @{SOURCE}({source_args})\n").unwrap();
        main.start_main(true, "");
        main.line(format!(
            "%ctx = call ptr @imre_dyn_new(i32 %argc, ptr %argv, ptr {config})"
        ));
        writeln!(
            output.functions,
            "declare void @{FILE}({physical_args})\ndeclare void @{DECLARED_FILE}({physical_args}, ptr)"
        )
        .unwrap();
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

    /// Attach one checked input to the module registered immediately before its inputs.
    pub(crate) fn register_single(
        &mut self,
        owner: &str,
        filename: &str,
        source: &str,
        flags: u32,
        code: &str,
    ) {
        if self.declarations.insert(SINGLE.to_owned()) {
            writeln!(
                self.output.functions,
                "declare void @{SINGLE}(ptr, ptr, ptr, ptr, i64, i32, ptr)"
            )
            .unwrap();
        }
        let text = self.output.data(source.as_bytes());
        writeln!(
            self.main.code,
            "  call void @{SINGLE}(ptr %ctx, ptr {owner}, ptr {filename}, ptr {text}, i64 {}, i32 {flags}, ptr {code})",
            source.len(),
        )
        .unwrap();
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
            self.main.code,
            "  call void @{ERROR}(ptr %ctx, ptr {name}, ptr {file}, ptr {text}, i64 {}, ptr {message}, ptr {position}, i8 {error_kind}, ptr {hash})",
            source.len(),
        )
        .unwrap();
    }

    pub(crate) fn finish(mut self) -> CodegenUnit {
        self.output.integer_prelude();
        self.main.finish_main("@imre_dyn_finish(ptr %ctx)");
        CodegenUnit {
            name: "__main__".into(),
            llvm_ir: format!(
                "{ABI}\n{}\n{}\n{}",
                self.output.constants, self.output.functions, self.main.code
            ),
        }
    }
}

/// The loader payload follows the initializer's constants in each output.
pub(crate) struct ModuleOperands<'a> {
    pub(crate) name: String,
    pub(crate) file: String,
    loader: &'a ModuleLoader<'a>,
}

impl ModuleOperands<'_> {
    pub(crate) fn write_call(&self, launcher: &mut ModuleLauncher, code: &str) {
        let name = &self.name;
        let file = &self.file;
        let mut declared_path = String::new();
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
                    PhysicalModuleKind::DeclaredFile(path) => {
                        let origin = launcher.output.data(path.as_os_str().as_encoded_bytes());
                        write!(declared_path, ", ptr {origin}").unwrap();
                        DECLARED_FILE
                    }
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
            launcher.main.code,
            "  call void @{function}(ptr %ctx, ptr {name}, ptr {file}, ptr {data}, {argument}, ptr {code}{declared_path})"
        )
        .unwrap();
    }
}
