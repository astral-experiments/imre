//! The read-only unidiff surface. Most hunk bodies are a single byte range in
//! the original string; blank context lines and newline markers can require
//! normalization. Parsed files and hunks retain their storage independently.

use super::{
    Error, Int, Kind, Object, Storage, StorageCell, Str, Words, acquire_rc, finish, object,
};
use std::{borrow::Cow, fmt::Write, ops::Range, ptr, rc::Rc};

pub(super) struct PatchSet {
    files: Rc<Object>,
    modified: Vec<Rc<Object>>,
}

pub(super) struct PatchedFile {
    path: Rc<Str>,
    hunks: Rc<Object>,
}

pub(super) struct Hunk {
    input: Rc<Str>,
    header: Str,
    source_start: Int,
    body: Range<usize>,
    normalized: Option<Str>,
    lines: usize,
}

impl Hunk {
    /// Merge adjacent original lines; copy only if their normalized text differs.
    fn append(&mut self, span: Range<usize>, replacement: Option<&str>) {
        if let Some(text) = replacement {
            let normalized = self
                .normalized
                .get_or_insert_with(|| Str::new(&self.input[self.body.clone()]));
            normalized.push_str(text);
        } else if let Some(normalized) = &mut self.normalized {
            normalized.push_str(&self.input[span.clone()]);
        }
        self.body.end = span.end;
        self.lines += 1;
    }

    fn text(&self) -> Str {
        let body = self
            .normalized
            .as_deref()
            .unwrap_or_else(|| &self.input[self.body.clone()]);
        let mut result = Str::with_capacity(self.header.len() + body.len());
        result.push_str(&self.header);
        result.push_str(body);
        result
    }
}

struct File<'a> {
    source: &'a str,
    target: &'a str,
    hunks: Vec<Hunk>,
    single_added: bool,
    single_removed: bool,
}

impl<'a> File<'a> {
    fn new(source: &'a str, target: &'a str) -> Self {
        Self {
            source,
            target,
            hunks: Vec::new(),
            single_added: false,
            single_removed: false,
        }
    }

    fn path(&self) -> Str {
        // unidiff compares paths after the two-character VCS prefixes and keeps
        // Git's surrounding quotes when stripping a prefix from the chosen name.
        let renamed = self.target != "/dev/null"
            && self.source.chars().skip(2).ne(self.target.chars().skip(2));
        let path = if self.source == "/dev/null" || renamed {
            self.target
        } else {
            self.source
        };
        let quoted = path
            .strip_prefix('"')
            .and_then(|path| path.strip_suffix('"'));
        let inner = quoted.unwrap_or(path);
        if matches!(
            inner.as_bytes(),
            [
                b'a' | b'b' | b'c' | b'i' | b'o' | b'w' | b'1' | b'2',
                b'/',
                ..
            ]
        ) {
            if quoted.is_some() {
                let mut result = Str::new("\"");
                result.push_str(&inner[2..]);
                result.push('"');
                result
            } else {
                Str::new(&inner[2..])
            }
        } else {
            Str::new(path)
        }
    }

    fn modified(&self) -> bool {
        self.source != "/dev/null"
            && self.target != "/dev/null"
            && !(self.hunks.len() == 1 && (self.single_added || self.single_removed))
    }
}

fn boxed(value: Storage) -> Rc<Object> {
    Rc::new(Object(StorageCell::new(value)))
}

fn sequence(items: impl Iterator<Item = Rc<Object>>) -> Rc<Object> {
    let mut words = Words::new(5);
    words
        .values
        .extend(items.map(|item| Rc::into_raw(item) as usize));
    words.into_object()
}

fn parse_error(message: impl Into<Str>) -> Error {
    Error::new(Kind::UnidiffParseError, message)
}

struct Lines<'a> {
    input: &'a str,
    offset: usize,
}

impl<'a> Iterator for Lines<'a> {
    type Item = (Range<usize>, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.input.get(self.offset..)?;
        if rest.is_empty() {
            return None;
        }
        let length = memchr::memchr(b'\n', rest.as_bytes()).map_or(rest.len(), |i| i + 1);
        let start = self.offset;
        self.offset += length;
        Some((start..self.offset, &rest[..length]))
    }
}

struct Span<'a> {
    start: &'a str,
    count: &'a str,
}

fn decimal(s: &str) -> Option<&str> {
    if s.is_empty() || !s.as_bytes().iter().all(u8::is_ascii_digit) {
        return None;
    }
    let normalized = s.trim_start_matches('0');
    Some(if normalized.is_empty() {
        "0"
    } else {
        normalized
    })
}

fn coordinates(s: &str) -> Option<Span<'_>> {
    let (start, count) = s.split_once(',').unwrap_or((s, "1"));
    Some(Span {
        start: decimal(start)?,
        count: decimal(count)?,
    })
}

/// The Python header grammar accepts Unicode decimal digits. Ordinary Git
/// coordinates stay borrowed; only normalize the coordinates, never the section.
fn normalize_header_digits(line: &str) -> Cow<'_, str> {
    let Some((coordinates, section)) = line.split_once(" @@") else {
        return Cow::Borrowed(line);
    };
    if coordinates.is_ascii() {
        return Cow::Borrowed(line);
    }
    let mut normal = String::with_capacity(line.len());
    for c in coordinates.chars() {
        normal.push(crate::integer_text::decimal_digit(c).map_or(c, |n| char::from(b'0' + n)));
    }
    normal.push_str(" @@");
    normal.push_str(section);
    Cow::Owned(normal)
}

fn header(line: &str) -> Option<(Span<'_>, Span<'_>, &str)> {
    let (source, rest) = line.strip_prefix("@@ -")?.split_once(" +")?;
    let (target, section) = rest.split_once(" @@")?;
    Some((
        coordinates(source)?,
        coordinates(target)?,
        section
            .strip_prefix(' ')
            .unwrap_or(section)
            .trim_end_matches('\n'),
    ))
}

fn git_paths(line: &str) -> Option<(&str, &str)> {
    let text = line
        .strip_prefix("diff --git ")?
        .split(['\t', '\n'])
        .next()?;
    if text.trim_start_matches('"').starts_with("a/")
        && let Some(i) = text.rmatch_indices(' ').find_map(|(i, _)| {
            let other = text[i + 1..].strip_prefix('"').unwrap_or(&text[i + 1..]);
            (other.starts_with("b/") && other.len() > 2).then_some(i)
        })
    {
        return Some((&text[..i], &text[i + 1..]));
    }
    if let Some((left, right)) = text
        .rmatch_indices(' ')
        .filter_map(|(i, _)| {
            let (l, r) = (&text[..i], &text[i + 1..]);
            (l.contains("://") && r.contains("://")).then_some((l, r))
        })
        .next()
    {
        return Some((left, right));
    }
    text.rsplit_once(' ')
        .filter(|(a, b)| !a.is_empty() && !b.is_empty())
}

fn binary_paths(line: &str) -> Option<(&str, &str)> {
    let text = line
        .strip_prefix("Binary files ")
        .or_else(|| line.strip_prefix("Binary file "))?;
    let paths = text
        .split_once(" differ")
        .or_else(|| text.split_once(" has changed"))?
        .0;
    let (source, target) = paths.split_once(" and ").unwrap_or((paths, paths));
    Some((source.split('\t').next()?, target.split('\t').next()?))
}

const NO_NEWLINE: &str = "\\ No newline at end of file";

fn parse(input: &Rc<Str>) -> Result<PatchSet, Error> {
    let mut files: Vec<File<'_>> = Vec::new();
    let mut current: Option<usize> = None;
    let mut last_source = None;
    let mut metadata = false;
    let mut lines = Lines {
        input: input.as_str(),
        offset: 0,
    };
    while let Some((span, line)) = lines.next() {
        if let Some((source, target)) = git_paths(line) {
            current = Some(files.len());
            files.push(File::new(source, target));
            metadata = true;
            continue;
        }
        let mode = |prefix| {
            line.strip_suffix('\n')
                .unwrap_or(line)
                .strip_prefix(prefix)
                .is_some_and(|s| {
                    !s.is_empty()
                        && s.chars()
                            .all(|c| crate::integer_text::decimal_digit(c).is_some())
                })
        };
        let added = mode("new file mode ");
        if added || mode("deleted file mode ") {
            let file = current.filter(|_| metadata).ok_or_else(|| {
                parse_error(format!(
                    "Unexpected {} file found: {line}",
                    if added { "new" } else { "deleted" }
                ))
            })?;
            if added {
                files[file].source = "/dev/null";
            } else {
                files[file].target = "/dev/null";
            }
            continue;
        }
        if let Some(source) = line.strip_prefix("--- ") {
            let source = source.split(['\t', '\n']).next().unwrap();
            last_source = Some(source);
            current = current.filter(|&i| files[i].source == source);
            continue;
        }
        if let Some(target) = line.strip_prefix("+++ ") {
            let target = target.split(['\t', '\n']).next().unwrap();
            if let Some(i) = current {
                if files[i].target != target {
                    return Err(parse_error(format!("Target without source: {line}")));
                }
            } else {
                let source = last_source
                    .ok_or_else(|| parse_error(format!("Target without source: {line}")))?;
                current = Some(files.len());
                files.push(File::new(source, target));
                metadata = false;
            }
            continue;
        }
        if line.starts_with("@@ -")
            && let adjusted = normalize_header_digits(line)
            && let Some((source, target, section)) = header(&adjusted)
        {
            let i = current.ok_or_else(|| parse_error(format!("Unexpected hunk found: {line}")))?;
            let count = |s: &str| s.parse::<usize>().unwrap_or(usize::MAX);
            let mut old = count(source.count);
            let mut new = count(target.count);
            let mut normalized_header = Str::with_capacity(line.len() + 5);
            write!(
                normalized_header,
                "@@ -{},{} +{},{} @@",
                source.start, source.count, target.start, target.count
            )
            .unwrap();
            if !section.is_empty() {
                normalized_header.push(' ');
                normalized_header.push_str(section);
            }
            normalized_header.push('\n');
            let start = source
                .start
                .parse::<usize>()
                .map(Int::from_usize)
                .unwrap_or_else(|_| Int::new(source.start.parse().expect("validated decimal")));
            let mut hunk = Hunk {
                input: Rc::clone(input),
                header: normalized_header,
                source_start: start,
                body: span.end..span.end,
                normalized: None,
                lines: 0,
            };
            // Each unit of source/target length must be accounted for; headers
            // and metadata are recognized only after both boundaries are reached.
            for (span, body) in lines.by_ref() {
                let mut replacement = None;
                let (take_old, take_new) = match body.as_bytes()[0] {
                    b'+' => (0, 1),
                    b'-' => (1, 0),
                    b' ' => (1, 1),
                    b'\\' => (0, 0),
                    b'\n' | b'\r' if body == "\n" || body == "\r\n" || body == "\r" => {
                        replacement = Some(if body == "\n" {
                            " \n"
                        } else if body == "\r\n" {
                            " \r\n"
                        } else {
                            " \r"
                        });
                        (1, 1)
                    }
                    _ => return Err(parse_error(format!("Hunk diff line expected: {body}"))),
                };
                old = old
                    .checked_sub(take_old)
                    .ok_or_else(|| parse_error("Hunk is longer than expected"))?;
                new = new
                    .checked_sub(take_new)
                    .ok_or_else(|| parse_error("Hunk is longer than expected"))?;
                hunk.append(span, replacement);
                if old == 0 && new == 0 {
                    break;
                }
            }
            if old != 0 || new != 0 {
                return Err(parse_error("Hunk is shorter than expected"));
            }
            files[i].single_added = source.start == "0" && source.count == "0";
            files[i].single_removed = target.start == "0" && target.count == "0";
            files[i].hunks.push(hunk);
            metadata = false;
            continue;
        }
        if line.starts_with(NO_NEWLINE) {
            let i = current.ok_or_else(|| parse_error(format!("Unexpected marker: {line}")))?;
            let hunk = files[i]
                .hunks
                .last_mut()
                .ok_or_else(|| parse_error("Unexpected marker: No newline at end of file"))?;
            let canonical = "\\ No newline at end of file\n";
            hunk.append(span, (line != canonical).then_some(canonical));
            continue;
        }
        if line == "\n"
            && let Some(hunk) = current.and_then(|i| files[i].hunks.last_mut())
        {
            hunk.append(span, None);
            continue;
        }
        if !metadata {
            current = None;
            metadata = true;
        }
        if let Some((source, target)) = binary_paths(line) {
            if current.is_none() {
                files.push(File::new(source, target));
            }
            metadata = false;
            current = None;
        } else if line == "GIT binary patch\n" {
            if current.is_none() {
                return Err(parse_error(format!(
                    "Unexpected binary patch marker: {line}"
                )));
            }
            metadata = false;
            current = None;
        }
    }
    let mut modified = Vec::new();
    let files = sequence(files.into_iter().map(|file| {
        let is_modified = file.modified();
        let path = Rc::new(file.path());
        let hunks = sequence(
            file.hunks
                .into_iter()
                .map(|hunk| boxed(Storage::Hunk(Box::new(hunk)))),
        );
        let result = boxed(Storage::PatchedFile(PatchedFile { path, hunks }));
        if is_modified {
            modified.push(Rc::clone(&result));
        }
        result
    }));
    Ok(PatchSet { files, modified })
}

/// # Safety
/// The input is a live Imre string. The result transfers one patch-set owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_patch_parse(text: *const Str) -> *const Object {
    // SAFETY: the caller roots this exact immutable string throughout acquisition.
    let input = unsafe { acquire_rc::<Str>(text as usize) };
    finish(
        parse(&input).map(|set| Rc::into_raw(boxed(Storage::PatchSet(set)))),
        ptr::null(),
    )
}

// All projections borrow a rooted object with its statically checked type and
// transfer an independent reference. Only the compiler can access the internal
// lists used for iteration; .modified_files returns a fresh Python list.
macro_rules! projection {
    ($name:ident, $node:ident, $out:ty, |$value:ident| $body:expr) => {
        /// # Safety
        /// The caller roots the exact, statically checked native object throughout the projection.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(node: *const Object) -> $out {
            // SAFETY: the caller roots the checked immutable node and projections cannot reenter.
            unsafe {
                object(node).0.with(|storage| match storage {
                    Storage::$node($value) => $body,
                    _ => unreachable!("checked unidiff projection"),
                })
            }
        }
    };
}
projection!(imre_patch_files, PatchSet, *const Object, |set| {
    Rc::into_raw(Rc::clone(&set.files))
});
projection!(imre_patch_modified_files, PatchSet, *const Object, |set| {
    Rc::into_raw(sequence(set.modified.iter().cloned()))
});
projection!(imre_patch_modified_count, PatchSet, usize, |set| {
    Int::from_usize(set.modified.len()).into_raw()
});
projection!(imre_patch_hunks, PatchedFile, *const Object, |file| {
    Rc::into_raw(Rc::clone(&file.hunks))
});
projection!(imre_patch_path, PatchedFile, *const Str, |file| {
    Rc::into_raw(Rc::clone(&file.path))
});
projection!(imre_patch_source_start, Hunk, usize, |hunk| hunk
    .source_start
    .clone()
    .into_raw());
projection!(imre_patch_hunk_text, Hunk, *const Str, |hunk| Rc::into_raw(
    Rc::new(hunk.text())
));

/// # Safety
/// The caller roots one of the three sealed list subclasses.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_patch_len(node: *const Object) -> usize {
    // SAFETY: the rooted node owns any inspected immutable sequence for this read.
    let len = unsafe {
        object(node).0.with(|storage| match storage {
            Storage::PatchSet(set) => set.files.0.with(|s| s.sequence().values.len()),
            Storage::PatchedFile(file) => file.hunks.0.with(|s| s.sequence().values.len()),
            Storage::Hunk(hunk) => hunk.lines,
            _ => unreachable!("checked unidiff length"),
        })
    };
    Int::from_usize(len).into_raw()
}
