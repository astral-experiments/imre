use super::sites::CheckedSites;
use super::{Function, RegisteredFrame};
use crate::frontend::{PhysicalOperand, RequiredSite, SourceEffect};
use ruff_text_size::TextSize;

impl<'a, 'p> crate::llvm::registered::PreparedSuiteEmitter<'a, 'p> for Function<'a, 'p> {
    fn frame_and_sites(
        &mut self,
        operand: Option<&crate::ir::Expr>,
    ) -> (&mut RegisteredFrame<'a, 'p>, &CheckedSites) {
        if let Some(operand) = operand {
            let (sites, mut definitions) = self.definitions();
            sites.place_operand(operand, &mut definitions);
        }
        (&mut self.frame, &self.sites)
    }

    fn place_definition(&mut self, site: TextSize) {
        let (sites, mut definitions) = self.definitions();
        sites.place(RequiredSite::Definition(site), &mut definitions);
    }

    fn place_statements(&mut self, body: &[crate::ir::Statement]) {
        let (sites, mut definitions) = self.definitions();
        sites.place_statements(body, &mut definitions);
    }

    fn emit_effect(
        &mut self,
        effect: &'p SourceEffect,
    ) -> Option<(&'p crate::ir::Statement, &'p [PhysicalOperand])> {
        let SourceEffect::Value(statement, inputs) = effect;
        Some((statement, inputs))
    }
}

impl<'p> Function<'_, 'p> {
    pub(super) fn declare_globals(&mut self, globals: &std::collections::BTreeSet<String>) {
        for name in globals {
            self.declared_globals.insert(name.to_owned());
            self.frame.declare_global(name);
        }
    }
}
