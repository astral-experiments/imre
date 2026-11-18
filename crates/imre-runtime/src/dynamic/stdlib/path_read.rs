//! Whole-file reads for an authenticated, unobserved PosixPath source frame.

use super::super::{
    Context, ffi,
    value::{self, Code, Error, Id, NONE, Result, SlotStorage, Value},
};

/// Enter after the ordinary Python signature binder has created the source frame.
///
/// # Safety
/// The compiler supplies a live entered scope and the original fallback body.
#[unsafe(no_mangle)]
pub(in crate::dynamic) unsafe extern "C" fn imre_dyn_path_read_bytes(
    raw: *mut Context,
    scope: Id,
    fallback: Code,
) -> Id {
    // SAFETY: this wrapper is emitted only for the authenticated bundled definition.
    let ctx = unsafe { &mut *raw };
    match attempt(ctx, scope) {
        Ok(Some(bytes)) => ffi::finish(ctx, Ok(bytes)),
        // SAFETY: the original body executes in the same entered frame.
        Ok(None) => unsafe { fallback(raw, scope) },
        Err(error) => ffi::finish(ctx, Err(error)),
    }
}

fn function(ctx: &Context, id: Id, root: Id, body: u32) -> bool {
    matches!(ctx.heap.get(id), Value::Function { code, scope, .. }
        if *scope == root && ctx.code_object(*code).definition.provider_body() == body as u8)
}

fn method(ctx: &Context, class: Id, root: Id, name: &str, body: u32) -> bool {
    ctx.class_attr(class, name)
        .is_some_and(|id| function(ctx, id, root, body))
}

fn classmethod(ctx: &Context, class: Id, root: Id, name: &str, body: u32) -> bool {
    ctx.class_attr(class, name).is_some_and(|id| {
        matches!(ctx.heap.get(id), Value::ClassMethod { callable, .. }
            if function(ctx, *callable, root, body))
    })
}

fn property(ctx: &Context, class: Id, root: Id, name: &str, body: u32) -> bool {
    ctx.class_attr(class, name).is_some_and(|id| {
        matches!(ctx.heap.get(id), Value::Property(property)
            if function(ctx, property.getter(), root, body))
    })
}

fn source_class(ctx: &Context, root: Id, id: Id, expected: &str, ancestors: &[Id]) -> bool {
    matches!(ctx.heap.get(id), Value::Class { metaclass: None, name, scope, ancestors: actual, .. }
        if name == expected && ctx.scope_data(*scope).parent == Some(root) && actual == ancestors)
}

fn binding(ctx: &Context, scope: Id, name: &str) -> Option<Id> {
    ctx.scalar_scope_name(scope, name)?
}

fn canonical_builtins(ctx: &Context, root: Id) -> bool {
    let Some(table) = ctx.source_builtins else {
        return false;
    };
    ["str", "len", "AttributeError"].iter().all(|name| {
        ctx.scalar_scope_name(root, name) == Some(None)
            && ctx.scalar_dict_name(table, name) == Some(ctx.builtins.get(*name).copied())
    })
}

fn module_scope(ctx: &Context, id: Id, name: &str) -> Option<Id> {
    if ctx.scalar_dict_name(ctx.modules, name) != Some(Some(id)) {
        return None;
    }
    match ctx.heap.get(id) {
        Value::Module {
            scope,
            constructor: None,
            ..
        } => Some(*scope),
        _ => None,
    }
}

fn native(ctx: &Context, id: Id, name: &str) -> bool {
    matches!(ctx.heap.get(id), Value::Native(actual) if actual == name)
}

fn slot(ctx: &Context, class: Id, owner: Id, name: &str) -> Option<Id> {
    let id = ctx.class_attr(class, name)?;
    match ctx.heap.get(id) {
        Value::SlotMember {
            owner: actual,
            name: spelling,
            storage: SlotStorage::Instance,
        } if *actual == owner
            && matches!(ctx.heap.get(*spelling), Value::Str(text) if text == name) =>
        {
            Some(id)
        }
        _ => None,
    }
}

fn slot_value(ctx: &Context, receiver: Id, member: Id) -> Option<Id> {
    let Value::Instance { member_values, .. } = ctx.heap.get(receiver) else {
        return None;
    };
    member_values.as_ref()?.get(&member).copied()
}

struct Slots {
    raw: Id,
    drive: Id,
    root: Id,
    tail: Id,
    text: Id,
}

fn environment(ctx: &Context, root: Id, receiver: Id) -> Option<Slots> {
    let Value::Instance { class, .. } = ctx.heap.get(receiver) else {
        return None;
    };
    let class = *class;
    let path = binding(ctx, root, "Path")?;
    let pure = binding(ctx, root, "PurePath")?;
    let posix = binding(ctx, root, "PurePosixPath")?;
    let concrete = binding(ctx, root, "PosixPath")?;
    let object = *ctx.builtins.get("object")?;
    if class != concrete
        || !source_class(ctx, root, pure, "PurePath", &[object])
        || !source_class(ctx, root, path, "Path", &[pure, object])
        || !source_class(ctx, root, posix, "PurePosixPath", &[pure, object])
        || !source_class(
            ctx,
            root,
            concrete,
            "PosixPath",
            &[path, posix, pure, object],
        )
        || ["__getattribute__", "__getattr__", "__setattr__", "__del__"]
            .iter()
            .any(|name| ctx.class_attr(class, name).is_some())
        || !canonical_builtins(ctx, root)
        || !method(ctx, class, root, "read_bytes", value::PATH_READ_BYTES_BODY)
        || !method(ctx, class, root, "open", value::PATH_OPEN_BODY)
        || !method(ctx, class, root, "__fspath__", value::PATH_FSPATH_BODY)
        || !method(ctx, class, root, "__str__", value::PATH_STR_BODY)
        || !classmethod(ctx, class, root, "_parse_path", value::PATH_PARSE_BODY)
        || !classmethod(
            ctx,
            class,
            root,
            "_format_parsed_parts",
            value::PATH_FORMAT_BODY,
        )
        || !property(ctx, class, root, "_raw_path", value::PATH_RAW_BODY)
        || !property(ctx, class, root, "drive", value::PATH_DRIVE_BODY)
        || !property(ctx, class, root, "root", value::PATH_ROOT_BODY)
        || !property(ctx, class, root, "_tail", value::PATH_TAIL_BODY)
    {
        return None;
    }
    let io = module_scope(ctx, binding(ctx, root, "io")?, "io")?;
    if binding(ctx, io, "open") != ctx.builtins.get("open").copied() {
        return None;
    }
    let parser = binding(ctx, root, "posixpath")?;
    if ctx.class_attr(class, "parser") != Some(parser) {
        return None;
    }
    let parser_scope = module_scope(ctx, parser, "posixpath")?;
    if !matches!(ctx.heap.get(binding(ctx, parser_scope, "sep")?), Value::Str(sep) if sep == "/")
        || binding(ctx, parser_scope, "altsep") != Some(NONE)
        || !native(
            ctx,
            binding(ctx, parser_scope, "splitroot")?,
            "posix._path_splitroot_ex",
        )
        || !native(
            ctx,
            binding(ctx, root, "_native_parts")?,
            "_imre_posixpath.normalized_parts",
        )
    {
        return None;
    }
    Some(Slots {
        raw: slot(ctx, class, pure, "_raw_paths")?,
        drive: slot(ctx, class, pure, "_drv")?,
        root: slot(ctx, class, pure, "_root")?,
        tail: slot(ctx, class, pure, "_tail_cached")?,
        text: slot(ctx, class, pure, "_str")?,
    })
}

/// Match the source's POSIX splitroot, normalized parts and formatted absolute spelling.
fn normalized_absolute(raw: &str) -> Option<(usize, Vec<&str>, String)> {
    if !raw.is_ascii() || !raw.starts_with('/') {
        return None;
    }
    let cut = crate::objects::posixpath::root_length(raw.as_bytes());
    let tail: Vec<_> = raw[cut..]
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let mut formatted = raw[..cut].to_owned();
    formatted.push_str(&tail.join("/"));
    Some((cut, tail, formatted))
}

fn spelling(ctx: &mut Context, receiver: Id, slots: Slots) -> Result<Option<Id>> {
    if let Some(text) = slot_value(ctx, receiver, slots.text) {
        return Ok(matches!(ctx.heap.get(text), Value::Str(path) if path.is_ascii() && path.starts_with('/')).then_some(text));
    }
    if [slots.drive, slots.root, slots.tail]
        .iter()
        .any(|&member| slot_value(ctx, receiver, member).is_some())
    {
        return Ok(None);
    }
    let Some(raw_paths) = slot_value(ctx, receiver, slots.raw) else {
        return Ok(None);
    };
    let Value::List(paths) = ctx.heap.get(raw_paths) else {
        return Ok(None);
    };
    let [raw] = paths.as_slice() else {
        return Ok(None);
    };
    let Value::Str(raw) = ctx.heap.get(*raw) else {
        return Ok(None);
    };
    let raw = raw.to_owned();
    let Some((cut, parts, formatted)) = normalized_absolute(&raw) else {
        return Ok(None);
    };
    ctx.heap
        .reserve_temporaries(parts.len().saturating_add(4))?;
    let drive = ctx.text("");
    let root = ctx.text(&raw[..cut]);
    let tail = parts.into_iter().map(|part| ctx.text(part)).collect();
    let tail = ctx.alloc(Value::List(tail));
    let text = ctx.text(formatted);
    ctx.write_slot_member(slots.drive, receiver, Some(drive))?;
    ctx.write_slot_member(slots.root, receiver, Some(root))?;
    ctx.write_slot_member(slots.tail, receiver, Some(tail))?;
    ctx.write_slot_member(slots.text, receiver, Some(text))?;
    Ok(Some(text))
}

fn attempt(ctx: &mut Context, scope: Id) -> Result<Option<Id>> {
    let root = ctx.root_scope(scope);
    let globals = ctx.scope_data(root);
    if globals.module != "pathlib"
        || globals.exec_globals.is_some()
        || !ctx.audit_hooks.is_empty()
        || ctx.heap.full_collection_active()
        || !super::difflib::unobserved_checkpoints(ctx)
    {
        return Ok(None);
    }
    let Some(receiver) = ctx.scope_name(scope, "self") else {
        return Ok(None);
    };
    let Some(slots) = environment(ctx, root, receiver) else {
        return Ok(None);
    };
    let Some(path) = spelling(ctx, receiver, slots)? else {
        return Ok(None);
    };
    let Value::Str(text) = ctx.heap.get(path) else {
        unreachable!()
    };
    match crate::objects::read_path_bytes_interruptible(text) {
        Ok(Some(bytes)) => Ok(Some(ctx.alloc(Value::Bytes(bytes)))),
        Ok(None) => Ok(Some(NONE)),
        Err(error) => {
            let has_filename = error
                .os_args()
                .and_then(|args| args.text_paths)
                .is_some_and(|paths| paths[0].is_some());
            let error = Error::from(error);
            Err(if has_filename {
                ctx.with_errno_filename(error, path)?
            } else {
                error
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalized_absolute;

    #[test]
    fn posix_absolute_normalization_matches_pathlib_roots_and_parts() {
        assert_eq!(
            normalized_absolute("//a/./b//"),
            Some((2, vec!["a", "b"], "//a/b".into()))
        );
        assert_eq!(
            normalized_absolute("///a/../b/"),
            Some((1, vec!["a", "..", "b"], "/a/../b".into()))
        );
        assert!(normalized_absolute("relative").is_none());
    }
}
