//! Per-invocation canonical JSON key owners. Each representation is the sole search storage.
use std::cmp::Ordering;
use std::hash::{BuildHasher, Hash, Hasher, RandomState};

use super::{Context, Id, NONE, ParsedString, Result, Value};

const SMALL: usize = 4;
const MAX_PROBES: usize = 32;
const NIL: u32 = u32::MAX;

/// A validated key can borrow its unescaped source spelling until a new Python owner is needed.
pub(super) enum KeySource<'a> {
    Borrowed(&'a str),
    Decoded(ParsedString),
}

#[derive(Clone, Copy)]
enum KeyView<'a> {
    Text(&'a str),
    Points(&'a [u32]),
}

impl KeySource<'_> {
    fn view(&self) -> KeyView<'_> {
        match self {
            Self::Borrowed(s) => KeyView::Text(s),
            Self::Decoded(ParsedString::Text(s)) => KeyView::Text(s),
            Self::Decoded(ParsedString::Surrogates(points)) => {
                debug_assert!(points.iter().any(|p| (0xd800..=0xdfff).contains(p)));
                KeyView::Points(points)
            }
        }
    }

    fn owner(self, ctx: &mut Context) -> Result<Id> {
        Ok(match self {
            Self::Borrowed(s) => ctx.text(crate::dynamic::copy_label(s)?),
            Self::Decoded(parsed) => parsed.boxed(ctx),
        })
    }
}

impl<'a> KeyView<'a> {
    fn owner(ctx: &'a Context, id: Id) -> Self {
        match ctx.heap.get(id) {
            Value::Str(text) => Self::Text(text),
            Value::SurrogateStr(points) => {
                debug_assert!(points.iter().any(|p| (0xd800..=0xdfff).contains(p)));
                Self::Points(points)
            }
            _ => unreachable!("canonical JSON key is an exact string"),
        }
    }

    fn compare(self, other: Self) -> Ordering {
        match (self, other) {
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
            (Self::Points(a), Self::Points(b)) => a.cmp(b),
            (Self::Text(a), Self::Points(b)) => a.chars().map(u32::from).cmp(b.iter().copied()),
            (Self::Points(a), Self::Text(b)) => a.iter().copied().cmp(b.chars().map(u32::from)),
        }
    }

    fn hash(self, seed: &RandomState) -> u64 {
        // Points contains an unpaired surrogate and therefore cannot equal valid Rust text.
        // Equal keys always have the same variant and hash, including escaped spellings.
        let mut h = seed.build_hasher();
        match self {
            Self::Text(text) => {
                0_u8.hash(&mut h);
                text.hash(&mut h);
            }
            Self::Points(points) => {
                1_u8.hash(&mut h);
                points.hash(&mut h);
            }
        }
        h.finish()
    }
}

struct Open {
    /// Occupied cells own the actual Python key ID; NONE is never a string key.
    cells: Vec<Id>,
    len: usize,
    seed: RandomState,
}

enum Probe {
    Found(Id),
    Empty(usize),
    Limit,
}

impl Open {
    fn cells(capacity: usize) -> Result<Vec<Id>> {
        let mut cells = Vec::new();
        cells
            .try_reserve_exact(capacity)
            .map_err(|_| crate::Error::allocation())?;
        cells.resize(capacity, NONE);
        Ok(cells)
    }

    fn new() -> Result<Self> {
        Ok(Self {
            cells: Self::cells(16)?,
            len: 0,
            seed: RandomState::new(),
        })
    }

    fn probe(&self, ctx: &Context, key: KeyView<'_>) -> Probe {
        let mut at = key.hash(&self.seed) as usize & (self.cells.len() - 1);
        for _ in 0..MAX_PROBES {
            let owner = self.cells[at];
            if owner == NONE {
                return Probe::Empty(at);
            }
            if key.compare(KeyView::owner(ctx, owner)) == Ordering::Equal {
                return Probe::Found(owner);
            }
            at = (at + 1) & (self.cells.len() - 1);
        }
        Probe::Limit
    }

    fn put(cells: &mut [Id], hash: u64, id: Id) -> bool {
        let mut at = hash as usize & (cells.len() - 1);
        for _ in 0..MAX_PROBES {
            if cells[at] == NONE {
                cells[at] = id;
                return true;
            }
            at = (at + 1) & (cells.len() - 1);
        }
        false
    }

    /// Build replacement primary storage before changing the current representation.
    fn grow(&mut self, ctx: &Context) -> Result<bool> {
        let capacity = self
            .cells
            .len()
            .checked_mul(2)
            .ok_or_else(crate::Error::allocation)?;
        let mut cells = Self::cells(capacity)?;
        for &id in &self.cells {
            if id != NONE && !Self::put(&mut cells, KeyView::owner(ctx, id).hash(&self.seed), id) {
                return Ok(false);
            }
        }
        self.cells = cells;
        Ok(true)
    }
}

struct Node {
    owner: Id,
    left: u32,
    right: u32,
    height: u8,
}
struct Tree {
    nodes: Vec<Node>,
    root: u32,
}
impl Tree {
    fn with_capacity(count: usize) -> Result<Self> {
        if count >= NIL as usize {
            return Err(crate::Error::allocation().into());
        }
        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(count)
            .map_err(|_| crate::Error::allocation())?;
        Ok(Self { nodes, root: NIL })
    }
    fn reserve_one(&mut self) -> Result<()> {
        if self.nodes.len() >= NIL as usize {
            return Err(crate::Error::allocation().into());
        }
        self.nodes
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation().into())
    }
    fn height(&self, n: u32) -> u8 {
        if n == NIL {
            0
        } else {
            self.nodes[n as usize].height
        }
    }
    fn update(&mut self, n: u32) {
        let node = &self.nodes[n as usize];
        let h = 1 + self.height(node.left).max(self.height(node.right));
        self.nodes[n as usize].height = h;
    }
    fn left(&mut self, n: u32) -> u32 {
        let r = self.nodes[n as usize].right;
        let t = self.nodes[r as usize].left;
        self.nodes[n as usize].right = t;
        self.nodes[r as usize].left = n;
        self.update(n);
        self.update(r);
        r
    }
    fn right(&mut self, n: u32) -> u32 {
        let l = self.nodes[n as usize].left;
        let t = self.nodes[l as usize].right;
        self.nodes[n as usize].left = t;
        self.nodes[l as usize].right = n;
        self.update(n);
        self.update(l);
        l
    }
    fn balance(&mut self, n: u32) -> u32 {
        self.update(n);
        let l = self.nodes[n as usize].left;
        let r = self.nodes[n as usize].right;
        let delta = i16::from(self.height(l)) - i16::from(self.height(r));
        if delta > 1 {
            let x = &self.nodes[l as usize];
            if self.height(x.left) < self.height(x.right) {
                let l = self.left(l);
                self.nodes[n as usize].left = l;
            }
            return self.right(n);
        }
        if delta < -1 {
            let x = &self.nodes[r as usize];
            if self.height(x.right) < self.height(x.left) {
                let r = self.right(r);
                self.nodes[n as usize].right = r;
            }
            return self.left(n);
        }
        n
    }
    fn find(&self, ctx: &Context, key: KeyView<'_>) -> Option<Id> {
        let mut at = self.root;
        while at != NIL {
            let n = &self.nodes[at as usize];
            match key.compare(KeyView::owner(ctx, n.owner)) {
                Ordering::Equal => return Some(n.owner),
                Ordering::Less => at = n.left,
                Ordering::Greater => at = n.right,
            }
        }
        None
    }
    /// Capacity has been reserved and the owner is not already in this tree.
    fn insert(&mut self, ctx: &Context, owner: Id) {
        self.root = self.insert_at(ctx, self.root, owner);
    }
    fn insert_at(&mut self, ctx: &Context, at: u32, owner: Id) -> u32 {
        if at == NIL {
            let id = self.nodes.len() as u32;
            self.nodes.push(Node {
                owner,
                left: NIL,
                right: NIL,
                height: 1,
            });
            return id;
        }
        let less = KeyView::owner(ctx, owner)
            .compare(KeyView::owner(ctx, self.nodes[at as usize].owner))
            == Ordering::Less;
        let next = if less {
            self.nodes[at as usize].left
        } else {
            self.nodes[at as usize].right
        };
        let child = self.insert_at(ctx, next, owner);
        if less {
            self.nodes[at as usize].left = child;
        } else {
            self.nodes[at as usize].right = child;
        }
        self.balance(at)
    }
}

enum State {
    Small { ids: [Id; SMALL], len: usize },
    Open(Open),
    Tree(Tree),
}
/// Sole authoritative per-document canonical owner storage; no copied spellings or secondary index.
pub(super) struct DocumentKeys {
    state: State,
}
impl Default for DocumentKeys {
    fn default() -> Self {
        Self {
            state: State::Small {
                ids: [NONE; SMALL],
                len: 0,
            },
        }
    }
}
impl DocumentKeys {
    fn promote_small(&mut self, ctx: &Context) -> Result<()> {
        let State::Small { ids, len } = &self.state else {
            unreachable!()
        };
        let mut open = Open::new()?;
        for &id in &ids[..*len] {
            let hash = KeyView::owner(ctx, id).hash(&open.seed);
            assert!(Open::put(&mut open.cells, hash, id));
            open.len += 1;
        }
        self.state = State::Open(open);
        Ok(())
    }
    fn promote_tree(&mut self, ctx: &Context) -> Result<()> {
        let State::Open(open) = &self.state else {
            unreachable!()
        };
        let count = open
            .len
            .checked_add(1)
            .ok_or_else(crate::Error::allocation)?;
        let mut tree = Tree::with_capacity(count)?;
        for &id in &open.cells {
            if id != NONE {
                tree.insert(ctx, id);
            }
        }
        self.state = State::Tree(tree);
        Ok(())
    }
    /// Return the exact canonical key owner and whether this invocation created it.
    pub(super) fn intern(
        &mut self,
        ctx: &mut Context,
        source: KeySource<'_>,
    ) -> Result<(Id, bool)> {
        loop {
            match &mut self.state {
                State::Small { ids, len } => {
                    if let Some(&id) = ids[..*len].iter().find(|&&id| {
                        source.view().compare(KeyView::owner(ctx, id)) == Ordering::Equal
                    }) {
                        return Ok((id, false));
                    }
                    if *len < SMALL {
                        let owner = source.owner(ctx)?;
                        ids[*len] = owner;
                        *len += 1;
                        return Ok((owner, true));
                    }
                    self.promote_small(ctx)?;
                }
                State::Open(open) => match open.probe(ctx, source.view()) {
                    Probe::Found(id) => return Ok((id, false)),
                    Probe::Limit => self.promote_tree(ctx)?,
                    Probe::Empty(at) => {
                        let limit = open.cells.len() - open.cells.len().div_ceil(3);
                        if open.len >= limit {
                            if !open.grow(ctx)? {
                                self.promote_tree(ctx)?;
                            }
                            continue;
                        }
                        let owner = source.owner(ctx)?;
                        open.cells[at] = owner;
                        open.len += 1;
                        return Ok((owner, true));
                    }
                },
                State::Tree(tree) => {
                    if let Some(id) = tree.find(ctx, source.view()) {
                        return Ok((id, false));
                    }
                    tree.reserve_one()?;
                    let owner = source.owner(ctx)?;
                    tree.insert(ctx, owner);
                    return Ok((owner, true));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), [] as [&str; 0])
    }

    #[test]
    fn decoded_equal_keys_reuse_the_exact_owner() {
        let mut ctx = context();
        let mut keys = DocumentKeys::default();
        let (long, fresh) = keys
            .intern(&mut ctx, KeySource::Borrowed("long_key_name"))
            .unwrap();
        assert!(fresh);
        let token = r#""long\u005fkey_name""#;
        let parsed = ParsedString::read(&ctx, token, None, 1..token.len() - 1)
            .ok()
            .unwrap();
        assert_eq!(
            keys.intern(&mut ctx, KeySource::Decoded(parsed)).unwrap(),
            (long, false)
        );

        let (emoji, _) = keys.intern(&mut ctx, KeySource::Borrowed("😀")).unwrap();
        let token = r#""\ud83d\ude00""#;
        let parsed = ParsedString::read(&ctx, token, None, 1..token.len() - 1)
            .ok()
            .unwrap();
        assert_eq!(
            keys.intern(&mut ctx, KeySource::Decoded(parsed)).unwrap(),
            (emoji, false)
        );

        let token = r#""\ud800""#;
        let parsed = ParsedString::read(&ctx, token, None, 1..token.len() - 1)
            .ok()
            .unwrap();
        let (surrogate, fresh) = keys.intern(&mut ctx, KeySource::Decoded(parsed)).unwrap();
        assert!(fresh);
        let parsed = ParsedString::read(&ctx, token, None, 1..token.len() - 1)
            .ok()
            .unwrap();
        assert_eq!(
            keys.intern(&mut ctx, KeySource::Decoded(parsed)).unwrap(),
            (surrogate, false)
        );
        assert!(
            matches!(ctx.heap.get(surrogate), Value::SurrogateStr(points) if points == &[0xd800])
        );
    }

    #[test]
    fn bounded_open_probes_promote_to_primary_avl_owners() {
        let mut ctx = context();
        let open = Open::new().unwrap();
        // The same low seven bits collide at every capacity used before the probe bound.
        let names: Vec<_> = (0_u64..)
            .map(|n| format!("collision_key_{n}"))
            .filter(|name| KeyView::Text(name).hash(&open.seed) & 127 == 0)
            .take(40)
            .collect();
        let mut keys = DocumentKeys {
            state: State::Open(open),
        };
        let mut ids = Vec::new();
        for name in &names {
            let (id, fresh) = keys.intern(&mut ctx, KeySource::Borrowed(name)).unwrap();
            assert!(fresh);
            ids.push(id);
        }
        let State::Tree(tree) = &keys.state else {
            panic!("probe bound must convert primary storage")
        };
        assert_eq!(tree.nodes.len(), names.len());
        assert!(tree.height(tree.root) <= 8);
        for (name, &id) in names.iter().zip(&ids).rev() {
            assert_eq!(
                keys.intern(&mut ctx, KeySource::Borrowed(name)).unwrap(),
                (id, false)
            );
        }
    }
}
