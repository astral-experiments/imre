//! Match byte literals and classes against the LC_CTYPE in effect at search time.
use super::*;
use regex_automata::util::captures::GroupInfo;
use regex_syntax::ast::{Ast, ClassSet, ClassSetItem, parse::Parser};
use regex_syntax::hir::{Class, ClassBytes, ClassBytesRange, Hir, Look};
use std::cell::RefCell;

#[derive(Debug, PartialEq)]
enum Atom {
    Literal(u8, bool),
    Class(ClassBytes, bool),
}

impl Atom {
    fn parse(ast: &Ast) -> Option<Self> {
        if let Ast::Literal(literal) = ast {
            return Some(Self::Literal(u8::try_from(literal.c).ok()?, false));
        }
        let Ast::ClassBracketed(class) = ast else {
            return None;
        };
        let ClassSet::Item(item) = &class.kind else {
            return None;
        };
        let items = match item {
            ClassSetItem::Union(union) => &union.items[..],
            item => std::slice::from_ref(item),
        };
        let mut ranges = Vec::with_capacity(items.len());
        let mut only_literals = true;
        for item in items {
            let (start, end) = match item {
                ClassSetItem::Literal(literal) => (literal.c, literal.c),
                ClassSetItem::Range(range) => {
                    only_literals = false;
                    (range.start.c, range.end.c)
                }
                _ => return None,
            };
            ranges.push(ClassBytesRange::new(
                u8::try_from(start).ok()?,
                u8::try_from(end).ok()?,
            ));
        }
        let bytes = ClassBytes::new(ranges);
        // Python deduplicates literals and gives [^a] a NOT_LITERAL opcode.
        // A degenerate range such as [^a-a] is still a (negated) set.
        if only_literals
            && let [range] = bytes.ranges()
            && range.start() == range.end()
        {
            return Some(Self::Literal(range.start(), class.negated));
        }
        Some(Self::Class(bytes, class.negated))
    }

    fn matches(&self, byte: u8, lower: u8, upper: u8) -> bool {
        match self {
            Self::Literal(c, negated) => (byte == *c || lower == *c || upper == *c) != *negated,
            Self::Class(set, negated) => {
                // IN_LOC_IGNORE tests the entire class (including its negation)
                // against the lowercase and uppercase byte separately.
                let member = |b| set.iter().any(|r| (r.start()..=r.end()).contains(&b)) != *negated;
                member(lower) || (lower != upper && member(upper))
            }
        }
    }
}

type CaseMap = [(u8, u8); 256];

pub(super) struct LocaleMatcher {
    atoms: Rc<[Atom]>,
    pub(super) groups: GroupInfo,
    full: bool,
    cached: RefCell<Option<(Box<CaseMap>, Regex)>>,
}

impl LocaleMatcher {
    pub(super) fn parse(source: &str, flags: i64, groups: &GroupInfo) -> Option<Self> {
        let spelling =
            imre_regex::locale_byte_source(source, flags & VERBOSE != 0, flags & DOTALL != 0)?;
        let ast = Parser::new().parse(&spelling).ok()?;
        let parts = match &ast {
            Ast::Concat(concat) => &concat.asts[..],
            atom => std::slice::from_ref(atom),
        };
        let atoms: Option<Vec<_>> = parts.iter().map(Atom::parse).collect();
        Some(Self {
            atoms: atoms?.into(),
            groups: groups.clone(),
            full: false,
            cached: RefCell::new(None),
        })
    }

    pub(super) fn same_program(&self, other: &Self) -> bool {
        self.atoms == other.atoms
    }

    pub(super) fn full(&self) -> Self {
        Self {
            atoms: self.atoms.clone(),
            groups: self.groups.clone(),
            full: true,
            cached: RefCell::new(None),
        }
    }

    pub(super) fn captures(
        &self,
        input: Input<'_>,
        captures: &mut Captures,
    ) -> std::result::Result<(), String> {
        let table: CaseMap = crate::sys::with_process_locale(|| {
            std::array::from_fn(|b| {
                // SAFETY: b is an unsigned char promoted to int; setlocale shares the lock.
                unsafe { (libc::tolower(b as i32) as u8, libc::toupper(b as i32) as u8) }
            })
        });
        let mut cached = self.cached.borrow_mut();
        if let Some((previous, regex)) = cached.as_ref()
            && **previous == table
        {
            regex.captures(input, captures);
            return Ok(());
        }

        let mut sequence = Vec::with_capacity(self.atoms.len() + usize::from(self.full));
        for atom in self.atoms.iter() {
            let members = (0..=u8::MAX).filter(|&b| {
                let (lo, hi) = table[usize::from(b)];
                atom.matches(b, lo, hi)
            });
            let class = ClassBytes::new(members.map(|b| ClassBytesRange::new(b, b)));
            sequence.push(Hir::class(Class::Bytes(class)));
        }
        if self.full {
            sequence.push(Hir::look(Look::End));
        }
        let regex = Regex::builder()
            .configure(Regex::config().utf8_empty(false))
            .build_from_hir(&Hir::concat(sequence))
            .map_err(|err| err.to_string())?;
        regex.captures(input, captures);
        *cached = Some((Box::new(table), regex));
        Ok(())
    }
}
