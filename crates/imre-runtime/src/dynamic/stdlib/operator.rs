use super::{Context, Error, Id, Result, Value};
use std::ops::RangeInclusive;

fn check_args(args: &[Id], kwargs: &[(String, Id)], arity: RangeInclusive<usize>) -> Result<()> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "attrgetter() takes no keyword arguments",
        ));
    }
    if !arity.contains(&args.len()) {
        return Err(Error::new(
            "TypeError",
            format!("attrgetter expected 1 argument, got {}", args.len()),
        ));
    }
    Ok(())
}

pub(super) fn attrgetter(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    check_args(args, kwargs, 1..=usize::MAX)?;
    let paths = args
        .iter()
        .map(|&id| {
            let Value::Str(name) = ctx.heap.get(id) else {
                return Err(Error::new("TypeError", "attribute name must be a string"));
            };
            Ok(name.split('.').map(str::to_owned).collect())
        })
        .collect::<Result<_>>()?;
    Ok(ctx.alloc(Value::AttrGetter(paths)))
}

impl Context {
    pub(in super::super) fn apply_attrgetter(
        &mut self,
        paths: &[Vec<String>],
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        check_args(args, kwargs, 1..=1)?;
        let mut lookup = |path: &[String]| {
            let value = path
                .iter()
                .try_fold(args[0], |obj, name| self.lookup_attr(obj, name))?;
            // A later descriptor may replace an earlier result and run the collector.
            self.heap.keep(value);
            Ok(value)
        };
        if let [path] = paths {
            return lookup(path);
        }
        let values = paths
            .iter()
            .map(|path| lookup(path))
            .collect::<Result<_>>()?;
        Ok(self.alloc(Value::Tuple(values)))
    }
}
