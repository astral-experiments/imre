//! Compile the class annotation provider from the declarations in the class body.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::dynamic::emit) struct Site {
    name: String,
    pub(in crate::dynamic::emit) expression: ast::Expr,
    conditional: Option<usize>,
}

#[derive(Default)]
pub(in crate::dynamic::emit) struct ClassSites {
    sites: BTreeMap<TextSize, Site>,
    unconditional: BTreeSet<TextSize>,
}

impl Function<'_, '_> {
    pub(in crate::dynamic::emit) fn annotated_class_body(&mut self, statements: &[ast::Stmt]) {
        let unconditional = statements.iter().map(Ranged::start).collect();
        self.class_sites = Some(ClassSites {
            unconditional,
            ..Default::default()
        });
        self.statements(statements);
        let ClassSites { mut sites, .. } = self.class_sites.take().unwrap();
        if self.body.terminated || sites.is_empty() {
            return;
        }

        let bindings = self.class_locals.take().expect("class body bindings");
        let (mut reads, complete) = self
            .source
            .scopes
            .annotations
            .class_reads(sites.keys(), &bindings);
        reads.insert("__classdict__".into());
        let offsets = if sites.values().any(|s| s.conditional.is_some()) {
            reads.insert("__conditional_annotations__".into());
            let offsets = self.op("list", &[]);
            for (index, (offset, site)) in sites
                .iter_mut()
                .filter(|(_, s)| s.conditional.is_some())
                .enumerate()
            {
                let offset = self.integer(offset.to_u32());
                self.op("append", &[&offsets, &offset]);
                site.conditional = Some(index);
            }
            offsets
        } else {
            "0".into()
        };
        let signature = self.frame.text(
            &json!({
                "posonly": ["format"], "scope_complete": complete, "scope_reads": reads,
                "code_flags": self.source.function_flags(None, self.in_function, false),
            })
            .to_string(),
        );
        let start = *sites.first_key_value().unwrap().0;
        let body = Body::ClassAnnotations(sites.into_values().collect(), bindings.globals);
        let code = self.source.definition(
            self.frame.output,
            String::new(),
            body,
            start,
            false,
            Entry::Compiled,
        );
        let environment = self.frame.op(
            "class_annotation_environment",
            &[&self.scope.clone(), &offsets],
        );
        let name = self.text("__annotate__");
        let qualname = self.frame.text(&format!("{}__annotate__", self.prefix));
        let defaults = self.op("dict", &[]);
        let function = self.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {environment}, i64 {name}, i64 {signature}, i64 {defaults}, i64 {qualname}, i64 0, i64 0, i1 false)"));
        self.bind("__annotate_func__", &function);
    }

    pub(super) fn remember_annotation(&mut self, assign: &ast::StmtAnnAssign, name: &str) {
        let sites = self.class_sites.as_mut().expect("class annotation site");
        let offset = assign.start();
        let conditional = (!sites.unconditional.contains(&offset)).then_some(0);
        sites.sites.insert(
            offset,
            Site {
                name: name.into(),
                expression: (*assign.annotation).clone(),
                conditional,
            },
        );
    }

    pub(in crate::dynamic::emit) fn class_annotation_values(
        &mut self,
        sites: &[Site],
        globals: &BTreeSet<String>,
    ) {
        self.check_annotation_format();
        let active = sites
            .iter()
            .any(|s| s.conditional.is_some())
            .then(|| self.lookup("__conditional_annotations__"));
        let dict = self.lookup("__classdict__");
        let local = self
            .frame
            .op("class_annotation_locals", &[&self.scope.clone(), &dict]);
        for name in globals {
            let name = self.code_name(name);
            self.op("declare_global", &[&local, &name]);
        }
        let result = self.op("dict", &[]);
        self.live.extend([local.clone(), result.clone()]);
        self.scope = local;
        for site in sites {
            let after = site.conditional.map(|index| {
                let index = self.integer(index);
                let present = self.op("in", &[&index, active.as_ref().unwrap()]);
                let truth = self.truth(&present);
                let yes = self.name("b");
                let after = self.name("b");
                self.branch(&truth, &yes, &after);
                self.label(&yes);
                after
            });
            self.checkpoint(&site.expression);
            let value = self.annotation_value(&site.expression);
            let key = self.code_name(&site.name);
            self.op("put", &[&result, &key, &value]);
            if let Some(after) = after {
                self.jump(&after);
                self.label(&after);
            }
        }
        self.end(&format!("ret i64 {result}"));
    }
}
