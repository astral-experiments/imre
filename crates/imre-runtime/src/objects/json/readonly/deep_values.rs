//! Iterative operations on documents emitted in postorder by the deep JSON decoder.
use super::{Error, Kind, Node, View, copy_entries, copy_text};

/// Copy an immutable subtree without walking it recursively. Children precede
/// their parent in encounter order, including when a subtree was copied before.
pub(super) fn copy(view: &View, root: usize, output: &mut Vec<Node>) -> Result<usize, Error> {
    let mut first = root;
    loop {
        let child = match &view.nodes[first] {
            Node::Array(items) => items.first().copied(),
            Node::Object(fields) => fields.first().map(|(_, i)| *i),
            _ => None,
        };
        match child {
            Some(i) => first = i,
            None => break,
        }
    }
    output
        .try_reserve(root - first + 1)
        .map_err(|_| Error::allocation())?;
    let start = output.len();
    let shift = |i: usize| start + (i - first);
    for node in &view.nodes[first..=root] {
        output.push(match node {
            Node::Array(items) => Node::Array(copy_entries(items, |&i| Ok(shift(i)))?),
            Node::Object(fields) => {
                Node::Object(fields.with_entries(copy_entries(fields, |(key, i)| {
                    Ok((copy_text(key)?, shift(*i)))
                })?))
            }
            Node::Datetime(text) => Node::Datetime(copy_text(text)?),
            _ => node.clone(),
        });
    }
    Ok(shift(root))
}

/// Compare a child at a time; a wide array needs only one pending frame.
pub(super) fn equal(left: &View, right: &View) -> Result<bool, Error> {
    let mut stack = vec![(left, left.index, right, right.index, 0)];
    while let Some((a, i, b, j, mut next)) = stack.pop() {
        let (a, i) = a.try_resolved(i)?;
        let (b, j) = b.try_resolved(j)?;
        let (l, r) = (&a.nodes[i], &b.nodes[j]);
        if std::ptr::eq(l, r) {
            continue;
        }
        let children = match (l, r) {
            (Node::Array(ls), Node::Array(rs)) => {
                if ls.len() != rs.len() {
                    return Ok(false);
                }
                ls.get(next).map(|child| (*child, rs[next]))
            }
            (Node::Object(ls), Node::Object(rs)) => {
                if next == 0 && rs.iter().any(|(key, _)| l.lookup(key).is_none()) {
                    return Ok(false);
                }
                while next < ls.len() && ls.is_shadowed(next) {
                    next += 1;
                }
                if let Some((key, value)) = ls.get(next) {
                    let Some(target) = r.lookup(key) else {
                        return Ok(false);
                    };
                    Some((*value, target))
                } else {
                    None
                }
            }
            (Node::Array(_) | Node::Object(_), _) | (_, Node::Array(_) | Node::Object(_)) => {
                return Ok(false);
            }
            _ if !a.equal_at(i, b, j)? => return Ok(false),
            _ => None,
        };
        if let Some((ai, bj)) = children {
            stack.try_reserve(2).map_err(|_| Error::allocation())?;
            stack.push((a, i, b, j, next + 1));
            stack.push((a, ai, b, bj, 0));
        }
    }
    Ok(true)
}

pub(super) fn write_repr(view: &View, out: &mut String) -> Result<(), Error> {
    let mut stack = vec![(view, view.index, 0)];
    while let Some((view, index, next)) = stack.pop() {
        let (view, index) = view.try_resolved(index)?;
        let node = &view.nodes[index];
        let (offset, child) = match node {
            Node::Array(items) => {
                if next == 0 {
                    out.push('[');
                }
                let Some(&child) = items.get(next) else {
                    out.push(']');
                    continue;
                };
                if next != 0 {
                    out.push_str(", ");
                }
                (next, child)
            }
            Node::Object(fields) => {
                if next == 0 {
                    out.push('{');
                }
                let mut offset = next;
                while offset < fields.len()
                    && fields[..offset]
                        .iter()
                        .any(|(key, _)| key == fields[offset].0)
                {
                    offset += 1;
                }
                let Some((key, _)) = fields.get(offset) else {
                    out.push('}');
                    continue;
                };
                if offset != 0 {
                    out.push_str(", ");
                }
                crate::objects::exceptions::push_string_repr(key, out);
                out.push_str(": ");
                (offset, node.lookup(key).unwrap())
            }
            _ => {
                view.write_at(index, out)?;
                continue;
            }
        };
        stack.try_reserve(2).map_err(|_| Error::allocation())?;
        stack.push((view, index, offset + 1));
        stack.push((view, child, 0));
    }
    Ok(())
}

/// TOML's recursive writer retains its own nesting limit. Check the selected
/// subtree before passing a newly accepted deep JSON view to that writer.
pub(super) fn check_toml_depth(view: &View) -> Result<(), Error> {
    let mut stack = vec![(view, view.index, 0)];
    while let Some((view, index, depth)) = stack.pop() {
        let (view, index) = view.try_resolved(index)?;
        match &view.nodes[index] {
            Node::Array(_) | Node::Object(_) if depth >= super::super::MAX_DEPTH => {
                return Err(Error::new(
                    Kind::ValueError,
                    "document nesting exceeds 128 containers",
                ));
            }
            Node::Array(items) => {
                stack
                    .try_reserve(items.len())
                    .map_err(|_| Error::allocation())?;
                stack.extend(items.iter().map(|i| (view, *i, depth + 1)));
            }
            Node::Object(fields) => {
                stack
                    .try_reserve(fields.len())
                    .map_err(|_| Error::allocation())?;
                stack.extend(fields.iter().map(|(_, i)| (view, *i, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}
