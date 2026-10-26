//! Mutable TOML/JSON views reuse native ordered dictionaries and list storage.
//! A subscript or shallow spread retains the same nested container owner.

use crate::StrOwner;
use ::toml::de::{DeTable, DeValue};
use std::collections::{HashMap, HashSet};

use super::{KeyMemo, Node, Nodes, View};
use crate::objects::mappings::{
    dict_set, imre_dict_contains, imre_dict_delete, imre_dict_get, imre_dict_new,
};
use crate::objects::{Error, Int, Kind, Object, Storage, StorageCell, Str, Words, object};

#[derive(Clone)]
pub(in crate::objects) enum Value {
    Null,
    Bool(bool),
    Integer(Int),
    Float(f64),
    String(StrOwner),
    Datetime(Str),
    Array(crate::ObjectOwner),
    Table(crate::ObjectOwner),
}

impl Value {
    pub(in crate::objects) fn object(self) -> crate::ObjectOwner {
        crate::ObjectOwner::new(Object(StorageCell::new(Storage::Document(self))))
    }
    fn word(self) -> usize {
        crate::ObjectOwner::into_raw(self.object()) as usize
    }
    fn array(
        values: impl ExactSizeIterator<Item = Result<crate::ObjectOwner, Error>>,
    ) -> Result<Self, Error> {
        let mut storage = Words::new(5);
        storage
            .values
            .try_reserve_exact(values.len())
            .map_err(|_| Error::allocation())?;
        for value in values {
            storage
                .values
                .push(crate::ObjectOwner::into_raw(value?) as usize);
        }
        Ok(Self::Array(storage.into_object()))
    }
    fn table(
        pairs: impl Iterator<Item = Result<(StrOwner, crate::ObjectOwner), Error>>,
    ) -> Result<Self, Error> {
        // SAFETY: a fresh string/object dictionary transfers one owner.
        let dict = unsafe { crate::ObjectOwner::from_raw(imre_dict_new(3, 5)) };
        for pair in pairs {
            let (key, value) = pair?;
            // SAFETY: the three owners root the exact mapping, string, and document.
            unsafe {
                dict_set(
                    crate::ObjectOwner::as_ptr(&dict),
                    StrOwner::as_ptr(&key) as usize,
                    crate::ObjectOwner::as_ptr(&value) as usize,
                )?
            };
        }
        Ok(Self::Table(dict))
    }
    pub(in crate::objects) fn items(source: &Object) -> Vec<(StrOwner, crate::ObjectOwner)> {
        // SAFETY: this operation copies all owners before traversing child values.
        unsafe { source.0.with(|v| v.dictionary().document_pairs()) }
    }
    pub(in crate::objects) fn elements(source: &Object) -> Vec<crate::ObjectOwner> {
        // SAFETY: only native list[document value] owners enter this path.
        unsafe {
            source.0.with(|s| {
                s.sequence()
                    .values
                    .iter()
                    .map(|&word| crate::ObjectOwner::acquire_word(word))
                    .collect()
            })
        }
    }
    fn snapshot(
        &self,
        nodes: &mut Vec<Node>,
        shared: &mut HashMap<(*const Object, usize), usize>,
        depth: usize,
    ) -> Result<usize, Error> {
        if depth >= super::super::MAX_DEPTH && matches!(self, Self::Array(_) | Self::Table(_)) {
            return Err(Error::new(
                Kind::ValueError,
                "document nesting exceeds 128 containers",
            ));
        }
        // Including the depth keeps the nesting check valid along every incoming path.
        let identity = match self {
            Self::Array(owner) | Self::Table(owner) => {
                Some((crate::ObjectOwner::as_ptr(owner), depth))
            }
            _ => None,
        };
        if let Some(index) = identity.and_then(|key| shared.get(&key)) {
            return Ok(*index);
        }
        let node = match self {
            Self::Null => Node::Null,
            Self::Bool(x) => Node::Bool(*x),
            Self::Integer(x) => Node::Integer(x.clone()),
            Self::Float(x) => Node::Float(*x),
            Self::String(x) => Node::String(x.clone()),
            Self::Datetime(x) => {
                Node::Datetime(Str::try_new(x.as_str()).map_err(|_| Error::allocation())?)
            }
            Self::Array(owner) => {
                // SAFETY: the document roots its exact list and every child throughout the snapshot.
                let children = unsafe {
                    owner.0.with(|s| {
                        let words = &s.sequence().values;
                        let mut children = Vec::new();
                        children
                            .try_reserve_exact(words.len())
                            .map_err(|_| Error::allocation())?;
                        for &word in words {
                            let value = Self::read(object(word as *const Object));
                            children.push(value.snapshot(nodes, shared, depth + 1)?);
                        }
                        Ok::<_, Error>(children)
                    })
                }?;
                Node::Array(children)
            }
            Self::Table(owner) => {
                let mut fields = Vec::new();
                // SAFETY: the source stays rooted and immutable while child indices are collected.
                unsafe {
                    owner.0.with(|s| {
                        for (name, word) in s.dictionary().document_entries() {
                            fields.try_reserve(1).map_err(|_| Error::allocation())?;
                            let key =
                                Str::try_new(name.as_str()).map_err(|_| Error::allocation())?;
                            let value = Self::read(object(word as *const Object));
                            fields.push((key, value.snapshot(nodes, shared, depth + 1)?));
                        }
                        Ok::<_, Error>(())
                    })
                }?;
                Node::Object(KeyMemo::default().fields(fields))
            }
        };
        nodes.try_reserve(1).map_err(|_| Error::allocation())?;
        let index = nodes.len();
        nodes.push(node);
        if let Some(key) = identity {
            shared.try_reserve(1).map_err(|_| Error::allocation())?;
            shared.insert(key, index);
        }
        Ok(index)
    }
    pub(in crate::objects) fn view(&self) -> Result<View, Error> {
        let mut nodes = Vec::new();
        let index = self.snapshot(&mut nodes, &mut HashMap::new(), 0)?;
        Ok(View {
            nodes: Nodes::Snapshot(
                crate::shared::Shared::try_new(nodes).map_err(|_| Error::allocation())?,
            ),
            index,
        })
    }
    fn from_json(view: &View, index: usize) -> Result<Self, Error> {
        Self::from_json_at(view, index, 0)
    }

    fn from_json_at(view: &View, index: usize, depth: usize) -> Result<Self, Error> {
        if index == view.index
            && let Some(root) = view.snapshot_root()?
        {
            return Self::from_json_at(&root, root.index, depth);
        }
        if depth >= super::super::MAX_DEPTH
            && matches!(&view.nodes[index], Node::Array(_) | Node::Object(_))
        {
            return Err(Error::new(
                Kind::ValueError,
                "document nesting exceeds 128 containers",
            ));
        }
        Ok(match &view.nodes[index] {
            Node::Deferred(array) => {
                let child = array.try_view()?;
                Self::from_json_at(child, child.index, depth)?
            }
            Node::Null => Self::Null,
            Node::Bool(x) => Self::Bool(*x),
            Node::Integer(x) => Self::Integer(x.clone()),
            Node::Float(x) => Self::Float(*x),
            Node::ConstantNan => Self::Float(f64::NAN),
            Node::String(x) => Self::String(x.clone()),
            Node::Datetime(x) => Self::Datetime(x.clone()),
            Node::Array(a) => Self::array(
                a.iter()
                    .map(|&index| Self::from_json_at(view, index, depth + 1).map(Self::object)),
            )?,
            Node::Object(t) => Self::table(t.iter().map(|(key, index)| {
                Ok((
                    t.owner(key)?,
                    Self::from_json_at(view, *index, depth + 1)?.object(),
                ))
            }))?,
        })
    }
    fn from_toml(node: &DeValue<'_>) -> Result<Self, Error> {
        fn string(value: &str) -> Result<StrOwner, Error> {
            let text = Str::try_new(value).map_err(|_| Error::allocation())?;
            StrOwner::try_new(text).map_err(|_| Error::allocation())
        }

        Ok(match node {
            DeValue::String(value) => Self::String(string(value.as_ref())?),
            DeValue::Integer(value) => Self::Integer(Int::new(
                num_bigint::BigInt::parse_bytes(value.as_str().as_bytes(), value.radix())
                    .ok_or_else(|| Error::new(Kind::TomlDecodeError, "invalid integer"))?,
            )),
            DeValue::Float(value) => Self::Float(
                value
                    .as_str()
                    .parse()
                    .map_err(|_| Error::new(Kind::TomlDecodeError, "invalid float"))?,
            ),
            DeValue::Boolean(value) => Self::Bool(*value),
            DeValue::Datetime(value) => {
                use std::fmt::Write;
                let mut iso = String::new();
                if let Some(date) = value.date {
                    write!(iso, "{date}").unwrap();
                }
                if let Some(time) = value.time {
                    if value.date.is_some() {
                        iso.push(' ');
                    }
                    let second = time.second.unwrap_or(0);
                    if second > 59 {
                        return Err(Error::new(Kind::TomlDecodeError, "second must be in 0..59"));
                    }
                    write!(iso, "{:02}:{:02}:{second:02}", time.hour, time.minute).unwrap();
                    let micros = time.nanosecond.unwrap_or(0) / 1000;
                    if micros != 0 {
                        write!(iso, ".{micros:06}").unwrap();
                    }
                }
                if let Some(offset) = value.offset {
                    let offset = offset.to_string();
                    iso.push_str(if offset == "Z" { "+00:00" } else { &offset });
                }
                Self::Datetime(Str::new(iso))
            }
            DeValue::Array(values) => Self::array(
                values
                    .iter()
                    .map(|v| Self::from_toml(v.get_ref()).map(Self::object)),
            )?,
            DeValue::Table(table) => {
                Self::table(table.iter().map(|(k, v)| {
                    Ok((string(k.get_ref())?, Self::from_toml(v.get_ref())?.object()))
                }))?
            }
        })
    }
    pub(in crate::objects) fn read(obj: &Object) -> Self {
        // SAFETY: callers hold a typed document owner. No storage borrow escapes.
        unsafe {
            obj.0.with(|storage| match storage {
                Storage::Document(node) => node.clone(),
                _ => unreachable!("expected document"),
            })
        }
    }
    fn list(&self) -> Result<crate::ObjectOwner, Error> {
        if let Self::Array(v) = self {
            Ok(v.clone())
        } else {
            Err(Error::new(Kind::TypeError, "expected a document list"))
        }
    }
    fn map(&self) -> Result<crate::ObjectOwner, Error> {
        if let Self::Table(v) = self {
            Ok(v.clone())
        } else {
            Err(Error::new(
                Kind::TypeError,
                "expected a document dictionary",
            ))
        }
    }
    pub(in crate::objects) fn truth(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Integer(i) => i.as_small() != Some(0),
            Self::Float(f) => *f != 0.0,
            Self::String(s) => !s.is_empty(),
            Self::Datetime(_) => true,
            // SAFETY: these owners keep the list or table alive for the length read.
            Self::Array(a) => unsafe { a.0.with(|s| !s.sequence().values.is_empty()) },
            // SAFETY: this owner pins a native dictionary with its original storage.
            Self::Table(t) => unsafe { t.0.with(|s| !s.dictionary().is_empty()) },
        }
    }
    fn check_acyclic(&self, target: &crate::ObjectOwner) -> Result<(), Error> {
        let (Self::Array(root) | Self::Table(root)) = self else {
            return Ok(());
        };
        let mut current = root.clone();
        let mut pending = Vec::new();
        let mut seen = HashSet::new();
        loop {
            if crate::ObjectOwner::ptr_eq(&current, target) {
                return Err(Error::new(
                    Kind::ValueError,
                    "cyclic document mutation is unsupported",
                ));
            }
            // SAFETY: the source graph roots every child throughout this read-only
            // traversal. Its array and table words are exact document owners.
            unsafe {
                current.0.with(|storage| {
                    let mut visit = |word| {
                        let (Self::Array(child) | Self::Table(child)) =
                            Self::read(object(word as *const Object))
                        else {
                            return Ok(());
                        };
                        let id = crate::ObjectOwner::as_ptr(&child);
                        if !seen.contains(&id) {
                            seen.try_reserve(1).map_err(|_| Error::allocation())?;
                            pending.try_reserve(1).map_err(|_| Error::allocation())?;
                            seen.insert(id);
                            pending.push(child);
                        }
                        Ok(())
                    };
                    match storage {
                        Storage::Sequence(items) => {
                            items.values.iter().copied().try_for_each(visit)
                        }
                        Storage::Dictionary(map) => {
                            map.document_entries().try_for_each(|(_, word)| visit(word))
                        }
                        _ => unreachable!("expected a document container"),
                    }
                })?;
            }
            let Some(next) = pending.pop() else {
                return Ok(());
            };
            current = next;
        }
    }
}

/// # Safety
/// The compiler pairs each opcode with three rooted words in its exact representations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_document(op: u32, a: usize, b: usize, c: usize) -> usize {
    // SAFETY: the opcode determines the concrete ABI types for raw inputs.
    let run = || unsafe {
        let object = |word: usize| object(word as *const Object);
        let doc = |word| Value::read(object(word));
        let str_ = |word| &*(word as *const Str);
        let ptr = |v: crate::ObjectOwner| crate::ObjectOwner::into_raw(v) as usize;
        Ok(match op {
            0 => Value::Null.word(),
            1 => Value::table(std::iter::empty())?.word(),
            2 => Value::array(std::iter::empty())?.word(),
            3 => Value::String(crate::StrOwner::acquire_word(a)).word(),
            4 => Value::Bool(a != 0).word(),
            5 => Value::Integer((*Int::borrow(a)).clone()).word(),
            6 => Value::Float(f64::from_bits(a as u64)).word(),
            7 => object(a).0.with(|s| {
                let Storage::Json(view) = s else {
                    unreachable!()
                };
                Value::from_json(view, view.index).map(Value::word)
            })?,
            8 => Value::Array(crate::ObjectOwner::acquire_word(a)).word(),
            9 => Value::Table(crate::ObjectOwner::acquire_word(a)).word(),
            10 => ptr(doc(a).map()?),
            11 => ptr(doc(a).list()?),
            12 => {
                let Value::String(s) = doc(a) else {
                    return Err(Error::new(Kind::TypeError, "expected a document string"));
                };
                StrOwner::into_raw(s) as usize
            }
            13 | 14 => {
                if op == 13 {
                    doc(a).map()?;
                } else {
                    doc(a).list()?;
                }
                ptr(crate::ObjectOwner::acquire_word(a))
            }
            15 => usize::from(matches!(doc(a), Value::Null)),
            16 => usize::from(matches!(doc(a), Value::Table(_))),
            17 => usize::from(matches!(doc(a), Value::Array(_))),
            18 => usize::from(matches!(doc(a), Value::String(_))),
            19 => {
                let mut repr = String::new();
                doc(a)
                    .view()?
                    .try_write_repr(&mut crate::objects::values::ReprWriter(&mut repr))?;
                crate::checked_owned_str(Str::from(repr))? as usize
            }
            20 => usize::from(doc(a).truth()),
            21 => {
                let parsed = DeTable::parse(str_(a))
                    .map_err(|e| Error::new(Kind::TomlDecodeError, e.to_string()))?;
                Value::from_toml(&DeValue::Table(parsed.into_inner()))?.word()
            }
            22 | 27 => {
                let view = doc(a).view()?;
                let mut text = String::new();
                let result = view.table(view.index, "", false, &mut text);
                if op == 27 {
                    object(b).0.with_mut(|s| {
                        let Storage::File(f) = s else { unreachable!() };
                        f.write_bytes(text.as_bytes())
                    })?;
                }
                result?;
                if op == 22 {
                    StrOwner::into_raw(
                        StrOwner::try_new(Str::from(text)).map_err(|_| Error::allocation())?,
                    ) as usize
                } else {
                    0
                }
            }
            23 => {
                let target = doc(a).map()?;
                let source = doc(b).map()?;
                for (key, val) in Value::items(&source) {
                    dict_set(
                        crate::ObjectOwner::as_ptr(&target),
                        StrOwner::as_ptr(&key) as usize,
                        crate::ObjectOwner::as_ptr(&val) as usize,
                    )?;
                }
                ptr(crate::ObjectOwner::acquire_word(a))
            }
            24 => {
                let target = doc(a).map()?;
                if imre_dict_contains(crate::ObjectOwner::as_ptr(&target), b) != 0 {
                    let old = imre_dict_get(crate::ObjectOwner::as_ptr(&target), b);
                    imre_dict_delete(crate::ObjectOwner::as_ptr(&target), b);
                    old
                } else {
                    ptr(crate::ObjectOwner::acquire_word(c))
                }
            }
            25 => {
                let target = doc(b).list()?;
                let values = Value::elements(object(a));
                for value in &values {
                    Value::read(value).check_acyclic(&target)?;
                }
                let old = target.0.with_mut(|s| {
                    let words = s.sequence_mut();
                    let new = values
                        .into_iter()
                        .map(|v| crate::ObjectOwner::into_raw(v) as usize)
                        .collect();
                    let old = std::mem::replace(&mut words.values, new);
                    words.update_header();
                    old
                });
                for word in old {
                    drop(crate::ObjectOwner::from_raw(word as *const Object));
                }
                0
            }
            26 => {
                let target = doc(b).map()?;
                doc(a).check_acyclic(&target)?;
                dict_set(crate::ObjectOwner::as_ptr(&target), c, a)?;
                0
            }
            28 => Value::table(
                object(a)
                    .0
                    .with(|s| s.dictionary().document_null_keys())
                    .into_iter()
                    .map(|s| Ok((s, Value::Null.object()))),
            )?
            .word(),
            29 => doc(a).view()?.into_handle() as usize,
            _ => unreachable!("valid document opcode"),
        })
    };
    crate::finish(run(), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_check_traverses_deep_shared_children_once() {
        let mut owners = vec![Words::new(5).into_object()];
        for _ in 0..8192 {
            let child = Value::Array(owners.last().unwrap().clone()).object();
            let mut parent = Words::new(5);
            parent
                .values
                .push(crate::ObjectOwner::into_raw(child.clone()) as usize);
            parent
                .values
                .push(crate::ObjectOwner::into_raw(child) as usize);
            owners.push(parent.into_object());
        }
        let root = Value::Array(owners.last().unwrap().clone());
        assert!(root.check_acyclic(&Words::new(5).into_object()).is_ok());
        assert!(root.check_acyclic(&owners[0]).is_err());
        drop(root);
        // Release parents before their independently rooted children.
        while owners.pop().is_some() {}
    }

    #[test]
    fn shared_children_reuse_the_same_snapshot_node() {
        let mut value = Value::array(std::iter::empty()).ok().expect("empty array");
        for _ in 0..22 {
            let child = value.object();
            value = Value::array([child.clone(), child].into_iter().map(Ok))
                .ok()
                .expect("two children");
        }
        let Ok(view) = value.view() else {
            panic!("shallow shared graph must fit")
        };
        let nodes = &*view.nodes;
        assert_eq!(nodes.len(), 23);
        for node in &nodes[1..] {
            assert!(matches!(node, Node::Array(items) if items.len() == 2 && items[0] == items[1]));
        }
    }

    #[test]
    fn a_shorter_cached_path_cannot_hide_excess_depth() {
        let mut shared = Value::Null;
        for _ in 0..80 {
            shared = Value::array(std::iter::once(Ok(shared.object())))
                .ok()
                .expect("one child");
        }
        let shared = shared.object();
        let mut long = shared.clone();
        for _ in 0..60 {
            long = Value::array(std::iter::once(Ok(long)))
                .ok()
                .expect("one child")
                .object();
        }
        let root = Value::array([shared, long].into_iter().map(Ok))
            .ok()
            .expect("two children");
        assert!(root.view().is_err());
    }

    #[test]
    fn mutable_snapshot_checks_container_depth() {
        let mut value = Value::Null;
        for _ in 0..super::super::super::MAX_DEPTH {
            value = Value::array(std::iter::once(Ok(value.object())))
                .ok()
                .expect("one child");
        }
        assert!(value.view().is_ok());
        let too_deep = Value::table(std::iter::once(Ok((
            StrOwner::new(Str::new("child")),
            value.object(),
        ))))
        .ok()
        .expect("one field");
        assert!(too_deep.view().is_err());
    }
}
