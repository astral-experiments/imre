//! Compile the no-progress check used by Python's nullable repetitions into the NFA.
//! Matching can then use the ordinary priority-ordered Pike VM, including for captures.
use super::assertions::{Guard, Kind};
use std::cell::RefCell;
use std::collections::HashMap;

use regex_automata::nfa::thompson::{BuildError, Builder, NFA, State, Transition, pikevm};
use regex_automata::util::captures::{Captures, GroupInfo};
use regex_automata::util::primitives::StateID;
use regex_automata::{Anchored, Input, PatternID, meta::Regex};
use regex_syntax::hir::{Capture, Hir, HirKind, Repetition};

pub(super) enum Matcher {
    Standard(Regex),
    #[cfg(target_os = "linux")]
    Locale(Box<super::locale::LocaleMatcher>),
    Lookahead(Box<super::lookahead::Lookahead>),
    Guarded(Box<GuardedMatcher>),
    Asserted(Box<AssertedMatcher>),
}

pub(super) struct GuardedMatcher {
    vm: pikevm::PikeVM,
    cache: RefCell<pikevm::Cache>,
}

pub(super) struct AssertedMatcher {
    candidate: Regex,
    nfa: NFA,
    groups: GroupInfo,
    slots: Vec<AssertionSlot>,
}

/// Internal capture instructions can also mark a zero-width assertion.
pub(super) enum AssertionSlot {
    Public(usize),
    Guard(Guard),
    Ignore,
}

impl AssertedMatcher {
    fn new(candidate: Regex, nfa: NFA, guards: &[(String, Guard)]) -> Result<Self, String> {
        let guards: HashMap<_, _> = guards
            .iter()
            .map(|(name, re)| (name.as_str(), re))
            .collect();
        let mut names = Vec::new();
        let mut slots = Vec::new();
        for name in nfa.group_info().pattern_names(PatternID::ZERO) {
            if let Some(guard) = name.and_then(|name| guards.get(name)) {
                if guard.kind == Kind::NegativeBehind {
                    // A successful negative guard leaves its captures unset. Reserve the
                    // public group numbers without mapping any NFA slots to those groups.
                    names.extend(
                        guard
                            .regex
                            .group_info()
                            .pattern_names(PatternID::ZERO)
                            .skip(1),
                    );
                }
                slots.push(AssertionSlot::Guard((**guard).clone()));
                slots.push(AssertionSlot::Ignore);
            } else {
                // Single-pattern NFAs have two consecutive slots per group, including group zero.
                let start = names.len() * 2;
                slots.push(AssertionSlot::Public(start));
                slots.push(AssertionSlot::Public(start + 1));
                names.push(name);
            }
        }
        let groups = GroupInfo::new([names]).map_err(|err| err.to_string())?;
        Ok(Self {
            candidate,
            nfa,
            slots,
            groups,
        })
    }

    /// An assertion-free match gives the earliest possible start for the guarded search.
    fn search(&self, mut input: Input<'_>, captures: &mut Captures, nonempty: bool) -> bool {
        if !nonempty && input.get_anchored() == Anchored::No && !input.is_done() {
            let Some(candidate) = self.candidate.find(input.clone()) else {
                captures.clear();
                return false;
            };
            // Keep the whole haystack so lookbehind can inspect bytes before this start.
            input.set_start(candidate.start());
        }
        super::split::search_nfa(&self.nfa, &self.slots, input, captures, nonempty)
    }
}

impl Matcher {
    /// Keep the usual matcher unless another optional iteration could start at the same offset.
    pub(super) fn new(
        regex: Regex,
        hir: Hir,
        utf8: bool,
        assertions: &[(String, Guard)],
    ) -> Result<Self, String> {
        let progress = needs_guard(&hir);
        if !progress && assertions.is_empty() {
            return Ok(Self::Standard(regex));
        }
        let size_limit = Regex::config().get_nfa_size_limit();
        let config = NFA::config().utf8(utf8).nfa_size_limit(size_limit);
        let nfa = if progress {
            let info = regex.group_info();
            let mut next_marker = info.group_len(PatternID::ZERO) as u32;
            let mut budget = size_limit.unwrap_or(usize::MAX);
            let marked = mark_repetitions(hir, &mut next_marker, &mut budget)?;
            let source = NFA::compiler()
                .configure(config)
                .build_from_hir(&marked)
                .map_err(|err| err.to_string())?;
            ProgressGraph::build(&source, info, size_limit).map_err(|err| err.to_string())?
        } else {
            NFA::compiler()
                .configure(config)
                .build_from_hir(&hir)
                .map_err(|err| err.to_string())?
        };
        if !assertions.is_empty() {
            return Ok(Self::Asserted(Box::new(AssertedMatcher::new(
                regex, nfa, assertions,
            )?)));
        }
        let vm = pikevm::PikeVM::new_from_nfa(nfa).map_err(|err| err.to_string())?;
        let cache = RefCell::new(vm.create_cache());
        Ok(Self::Guarded(Box::new(GuardedMatcher { vm, cache })))
    }

    /// Share the progress-aware automaton with the nonempty retry used by split and findall.
    pub(super) fn guarded_nfa(&self) -> Option<&NFA> {
        match self {
            Self::Guarded(matcher) => Some(matcher.vm.get_nfa()),
            _ => None,
        }
    }

    /// Share the assertion guards and public capture-slot mapping with a replay.
    pub(super) fn asserted_nfa(&self) -> Option<(&NFA, &[AssertionSlot])> {
        match self {
            Self::Asserted(matcher) => Some((&matcher.nfa, &matcher.slots)),
            _ => None,
        }
    }

    pub(super) fn group_info(&self) -> &GroupInfo {
        match self {
            #[cfg(target_os = "linux")]
            Self::Locale(matcher) => &matcher.groups,
            Self::Standard(regex) => regex.group_info(),
            Self::Asserted(matcher) => &matcher.groups,
            Self::Lookahead(matcher) => &matcher.groups,
            Self::Guarded(matcher) => matcher.vm.get_nfa().group_info(),
        }
    }

    /// Allocate capture slots that can also be reused for a nonempty retry.
    pub(super) fn create_captures(&self) -> Captures {
        Captures::all(self.group_info().clone())
    }

    /// Run the search directly when a zero-width instruction depends on the subject.
    pub(super) fn asserted_search(
        &self,
        input: Input<'_>,
        captures: &mut Captures,
        nonempty: bool,
    ) -> Result<Option<bool>, String> {
        let found = match self {
            Self::Asserted(matcher) => matcher.search(input, captures, nonempty),
            Self::Lookahead(matcher) => {
                matcher.captures(input, nonempty, captures)?;
                captures.is_match()
            }
            _ => return Ok(None),
        };
        Ok(Some(found))
    }

    /// Store the preferred match and its capture spans for this search range.
    pub(super) fn captures(&self, input: Input<'_>, captures: &mut Captures) -> Result<(), String> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Locale(matcher) => return matcher.captures(input, captures),
            Self::Standard(regex) => regex.captures(input, captures),
            Self::Asserted(_) | Self::Lookahead(_) => {
                return self.asserted_search(input, captures, false).map(|_| ());
            }
            Self::Guarded(matcher) => {
                matcher
                    .vm
                    .captures(&mut matcher.cache.borrow_mut(), input, captures)
            }
        }
        Ok(())
    }
}

/// A required iteration may be empty. Only consecutive optional iterations need the check.
fn can_stall(repeat: &Repetition) -> bool {
    // An unreachable alternative can hide the minimum width of its siblings.
    repeat.sub.properties().minimum_len().unwrap_or(0) == 0
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
