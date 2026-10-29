//! Compile the no-progress check used by Python's nullable repetitions into the NFA.
//! Matching can then use the ordinary priority-ordered Pike VM, including for captures.
use std::cell::RefCell;
use std::collections::HashMap;

use regex_automata::nfa::thompson::{BuildError, Builder, NFA, State, Transition, pikevm};
use regex_automata::util::captures::{Captures, GroupInfo};
use regex_automata::util::primitives::StateID;
use regex_automata::{Input, PatternID, meta::Regex};
use regex_syntax::hir::{Capture, Hir, HirKind, Repetition};

pub(super) enum Matcher {
    Standard(Regex),
    Guarded(Box<GuardedMatcher>),
}

pub(super) struct GuardedMatcher {
    vm: pikevm::PikeVM,
    cache: RefCell<pikevm::Cache>,
}

impl Matcher {
    /// Keep the usual matcher unless another optional iteration could start at the same offset.
    pub(super) fn new(regex: Regex, hir: Hir, utf8: bool) -> Result<Self, String> {
        if !needs_guard(&hir) {
            return Ok(Self::Standard(regex));
        }
        let info = regex.group_info();
        let mut next_marker = info.group_len(PatternID::ZERO) as u32;
        let size_limit = Regex::config().get_nfa_size_limit();
        let mut budget = size_limit.unwrap_or(usize::MAX);
        let marked = mark_repetitions(hir, &mut next_marker, &mut budget)?;
        let source = NFA::compiler()
            .configure(NFA::config().utf8(utf8).nfa_size_limit(size_limit))
            .build_from_hir(&marked)
            .map_err(|err| err.to_string())?;
        let nfa = ProgressGraph::build(&source, info, size_limit).map_err(|err| err.to_string())?;
        let vm = pikevm::PikeVM::new_from_nfa(nfa).map_err(|err| err.to_string())?;
        let cache = RefCell::new(vm.create_cache());
        Ok(Self::Guarded(Box::new(GuardedMatcher { vm, cache })))
    }

    /// Share the progress-aware automaton with the nonempty retry used by split and findall.
    pub(super) fn guarded_nfa(&self) -> Option<&NFA> {
        match self {
            Self::Standard(_) => None,
            Self::Guarded(matcher) => Some(matcher.vm.get_nfa()),
        }
    }

    /// Return the original pattern's numbered and named capture groups.
    pub(super) fn group_info(&self) -> &GroupInfo {
        match self {
            Self::Standard(regex) => regex.group_info(),
            Self::Guarded(matcher) => matcher.vm.get_nfa().group_info(),
        }
    }

    /// Allocate capture slots that can also be reused for a nonempty retry.
    pub(super) fn create_captures(&self) -> Captures {
        Captures::all(self.group_info().clone())
    }

    /// Store the preferred match and its capture spans for this search range.
    pub(super) fn captures(&self, input: Input<'_>, captures: &mut Captures) {
        match self {
            Self::Standard(regex) => regex.captures(input, captures),
            Self::Guarded(matcher) => {
                matcher
                    .vm
                    .captures(&mut matcher.cache.borrow_mut(), input, captures)
            }
        }
    }
}

/// A required iteration may be empty. Only consecutive optional iterations need the check.
fn can_stall(repeat: &Repetition) -> bool {
    repeat.sub.properties().minimum_len() == Some(0)
        && repeat.max.is_none_or(|max| max - repeat.min > 1)
}

/// Check the whole expression for a repeat whose operand can match empty.
fn needs_guard(hir: &Hir) -> bool {
    matches!(hir.kind(), HirKind::Repetition(repeat) if can_stall(repeat))
        || hir.kind().subs().iter().any(needs_guard)
}

/// Insert private capture instructions to label the start of each optional iteration and its exit.
fn mark_repetitions(
    hir: Hir,
    next_marker: &mut u32,
    budget: &mut usize,
) -> Result<Hir, &'static str> {
    const SIZE_ERROR: &str = "nullable repetition exceeds the regex size limit";
    let cost = size_of::<Hir>()
        + match hir.kind() {
            HirKind::Literal(literal) => literal.0.len(),
            HirKind::Class(regex_syntax::hir::Class::Unicode(class)) => size_of_val(class.ranges()),
            HirKind::Class(regex_syntax::hir::Class::Bytes(class)) => size_of_val(class.ranges()),
            HirKind::Capture(capture) => capture.name.as_deref().map_or(0, str::len),
            _ => 0,
        };
    *budget = budget.checked_sub(cost).ok_or(SIZE_ERROR)?;
    let mut rewrite = |child| mark_repetitions(child, next_marker, budget);
    Ok(match hir.into_kind() {
        HirKind::Empty => Hir::empty(),
        HirKind::Literal(literal) => Hir::literal(literal.0),
        HirKind::Class(class) => Hir::class(class),
        HirKind::Look(look) => Hir::look(look),
        HirKind::Concat(parts) => {
            Hir::concat(parts.into_iter().map(rewrite).collect::<Result<_, _>>()?)
        }
        HirKind::Alternation(parts) => {
            Hir::alternation(parts.into_iter().map(rewrite).collect::<Result<_, _>>()?)
        }
        HirKind::Capture(mut capture) => {
            capture.sub = Box::new(rewrite(*capture.sub)?);
            Hir::capture(capture)
        }
        HirKind::Repetition(repeat) => {
            let guarded = can_stall(&repeat);
            let before = *budget;
            let sub = mark_repetitions(*repeat.sub, next_marker, budget)?;
            if !guarded {
                return Ok(Hir::repetition(Repetition {
                    sub: Box::new(sub),
                    ..repeat
                }));
            }
            let enter = *next_marker;
            *next_marker += 2;
            let marker = |index| {
                Hir::capture(Capture {
                    index,
                    name: None,
                    sub: Box::new(Hir::empty()),
                })
            };
            let mut parts = Vec::new();
            if repeat.min != 0 {
                // Nested lower bounds duplicate their operand. Charge the copy before cloning it.
                *budget = budget.checked_sub(before - *budget).ok_or(SIZE_ERROR)?;
                parts.push(Hir::repetition(Repetition {
                    min: repeat.min,
                    max: Some(repeat.min),
                    greedy: repeat.greedy,
                    sub: Box::new(sub.clone()),
                }));
            }
            // Two empty marker captures, the optional repetition and its enclosing sequences.
            *budget = budget.checked_sub(7 * size_of::<Hir>()).ok_or(SIZE_ERROR)?;
            parts.push(Hir::repetition(Repetition {
                min: 0,
                max: repeat.max.map(|max| max - repeat.min),
                greedy: repeat.greedy,
                sub: Box::new(Hir::concat(vec![marker(enter), sub])),
            }));
            parts.push(marker(enter + 1));
            Hir::concat(parts)
        }
    })
}

/// Each output state remembers which enclosing iterations started at the current input offset.
/// Consuming any byte clears that stack. Identical stacks and output states share storage.
struct ProgressGraph<'a> {
    source: &'a NFA,
    groups: &'a GroupInfo,
    out: Builder,
    states: HashMap<(StateID, usize), StateID>,
    pending: Vec<(StateID, usize, StateID)>,
    stacks: Vec<(usize, usize)>,
    pushes: HashMap<(usize, usize), usize>,
}

impl<'a> ProgressGraph<'a> {
    /// Copy reachable states from both search entries, omitting paths that repeat without progress.
    fn build(
        source: &'a NFA,
        groups: &'a GroupInfo,
        limit: Option<usize>,
    ) -> Result<NFA, Box<BuildError>> {
        let mut graph = Self {
            source,
            groups,
            out: Builder::new(),
            states: HashMap::new(),
            pending: Vec::new(),
            stacks: vec![(0, usize::MAX)],
            pushes: HashMap::new(),
        };
        graph.out.set_utf8(source.is_utf8());
        graph.out.set_look_matcher(source.look_matcher().clone());
        graph.out.set_size_limit(limit)?;
        graph.out.start_pattern()?;
        // Graph traversal can reach captures out of declaration order. Register the public names first.
        let dead = graph.out.add_fail()?;
        for (index, name) in groups.pattern_names(PatternID::ZERO).enumerate() {
            graph
                .out
                .add_capture_start(dead, index as u32, name.map(Into::into))?;
        }
        let anchored = graph.visit(source.start_anchored(), 0)?;
        let unanchored = graph.visit(source.start_unanchored(), 0)?;
        while let Some((state, stack, output)) = graph.pending.pop() {
            let target = graph.copy_state(state, stack)?;
            graph.out.patch(output, target)?;
        }
        graph.out.finish_pattern(anchored)?;
        Ok(graph.out.build(anchored, unanchored)?)
    }

    /// Reserve each reachable state before following its edges, including edges that loop back to it.
    fn visit(&mut self, state: StateID, stack: usize) -> Result<StateID, Box<BuildError>> {
        if let Some(&output) = self.states.get(&(state, stack)) {
            return Ok(output);
        }
        let output = self.out.add_empty()?;
        self.states.insert((state, stack), output);
        self.pending.push((state, stack, output));
        Ok(output)
    }

    /// Keep priority, look assertions, and capture slots while enforcing the private repetition markers.
    fn copy_state(&mut self, state: StateID, stack: usize) -> Result<StateID, Box<BuildError>> {
        Ok(match self.source.state(state) {
            State::ByteRange { trans } => {
                let next = self.visit(trans.next, 0)?;
                self.out.add_range(Transition { next, ..*trans })
            }
            State::Sparse(trans) => {
                let mut edges = Vec::with_capacity(trans.transitions.len());
                for edge in &*trans.transitions {
                    edges.push(Transition {
                        next: self.visit(edge.next, 0)?,
                        ..*edge
                    });
                }
                self.out.add_sparse(edges)
            }
            State::Dense(trans) => {
                let mut edges = Vec::new();
                for byte in 0..=u8::MAX {
                    if let Some(next) = trans.matches_byte(byte) {
                        edges.push(Transition {
                            start: byte,
                            end: byte,
                            next: self.visit(next, 0)?,
                        });
                    }
                }
                self.out.add_sparse(edges)
            }
            State::Look { look, next } => {
                let target = self.visit(*next, stack)?;
                self.out.add_look(target, *look)
            }
            State::Union { alternates } => {
                let edges = alternates
                    .iter()
                    .map(|&s| self.visit(s, stack))
                    .collect::<Result<_, _>>()?;
                self.out.add_union(edges)
            }
            State::BinaryUnion { alt1, alt2 } => {
                let first = self.visit(*alt1, stack)?;
                let second = self.visit(*alt2, stack)?;
                self.out.add_union(vec![first, second])
            }
            State::Capture {
                next,
                group_index,
                slot,
                ..
            } => {
                let index = group_index.as_usize();
                let open =
                    self.source.group_info().slot(PatternID::ZERO, index) == Some(slot.as_usize());
                if let Some(marker) = index.checked_sub(self.groups.group_len(PatternID::ZERO)) {
                    if !open {
                        return self.visit(*next, stack);
                    }
                    let repeat = marker / 2;
                    let (parent, last) = self.stacks[stack];
                    let active = if marker.is_multiple_of(2) {
                        if last == repeat {
                            return Ok(self.out.add_fail()?);
                        }
                        *self.pushes.entry((stack, repeat)).or_insert_with(|| {
                            let id = self.stacks.len();
                            self.stacks.push((stack, repeat));
                            id
                        })
                    } else if last == repeat {
                        parent
                    } else {
                        stack
                    };
                    return self.visit(*next, active);
                } else {
                    let target = self.visit(*next, stack)?;
                    if open {
                        self.out.add_capture_start(
                            target,
                            index as u32,
                            self.groups.to_name(PatternID::ZERO, index).map(Into::into),
                        )
                    } else {
                        self.out.add_capture_end(target, index as u32)
                    }
                }
            }
            State::Match { .. } => self.out.add_match(),
            State::Fail => self.out.add_fail(),
        }?)
    }
}
