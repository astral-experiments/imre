//! POSIX lexical path normalization for Python file operands.
use std::collections::TryReserveError;

/// Exactly two leading slashes form a distinct root on POSIX.
pub fn root_length<T: Copy + Eq + From<u8>>(path: &[T]) -> usize {
    let slash = T::from(b'/');
    if path.starts_with(&[slash; 2]) && !path.starts_with(&[slash; 3]) {
        2
    } else {
        usize::from(path.starts_with(&[slash]))
    }
}

/// Normalize components lexically. For relative abspath, supply the absolute cwd;
/// for an absolute path or for normpath, pass none.
/// The output buffer doubles as the component stack, so arbitrary bytes need no decoding.
pub fn normalize<T: Copy + Eq + From<u8>>(
    path: &[T],
    cwd: Option<&[T]>,
) -> Result<Vec<T>, TryReserveError> {
    let slash = T::from(b'/');
    let dot = T::from(b'.');
    let root = root_length(cwd.unwrap_or(path));
    let mut output = Vec::new();
    output.try_reserve(root.max(1))?;
    output.resize(root, slash);
    for part in cwd
        .into_iter()
        .chain([path])
        .flat_map(|p| p.split(|&b| b == slash))
    {
        match part {
            [] => continue,
            [one] if *one == dot => continue,
            [a, b] if *a == dot && *b == dot => {
                let tail = &output[root..];
                let last = tail.rsplit(|&b| b == slash).next().unwrap_or_default();
                if !last.is_empty() && last != [dot; 2] {
                    let end = root + (tail.len() - last.len()).saturating_sub(1);
                    output.truncate(end);
                    continue;
                }
                if root != 0 {
                    continue;
                }
            }
            _ => {}
        }
        let separator = output.len() > root;
        output.try_reserve(part.len() + usize::from(separator))?;
        if separator {
            output.push(slash);
        }
        output.extend_from_slice(part);
    }
    if output.is_empty() {
        output.push(dot);
    }
    Ok(output)
}
