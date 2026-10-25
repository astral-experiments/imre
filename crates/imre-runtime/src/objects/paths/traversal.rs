//! Lazy POSIX glob selectors. Directory snapshots own names and release the descriptor before yielding.

use regex_automata::meta::Regex;
use std::ffi::OsString;
use std::fs;

use super::{Error, Kind, Pattern, Result, Str, join, matching::glob_regex, snapshot};

enum Part {
    Literal(Str),
    Parent,
    Directory,
    Wild(NameMatch),
    Recursive(Option<Regex>),
}

enum NameMatch {
    Sensitive(Pattern),
    Folded(Regex),
}

impl NameMatch {
    fn matches(&self, name: &str) -> bool {
        match self {
            Self::Sensitive(pattern) => pattern.matches(name),
            Self::Folded(regex) => regex.is_match(name),
        }
    }
}

type Names = std::vec::IntoIter<OsString>;

// Each continuation can suspend while a later selector yields. Recursive scans
// visit all entries in their directory before popping the last discovered subdirectory.
enum Work {
    Select {
        path: Str,
        part: usize,
        known: bool,
    },
    Children {
        path: Str,
        part: usize,
        names: Names,
    },
    Descendants {
        part: usize,
        scan: Scan,
    },
}

struct Scan {
    root: Str,
    stack: Vec<Str>,
    entries: Option<(Str, Names)>,
}

fn string_name(name: OsString) -> Result<String> {
    name.into_string().map_err(|_| {
        Error::new(
            Kind::UnicodeDecodeError,
            "directory entry is not valid UTF-8",
        )
    })
}

fn is_dir(path: &str, follow: bool) -> bool {
    let metadata = if follow {
        fs::metadata(path)
    } else {
        fs::symlink_metadata(path)
    };
    metadata.is_ok_and(|info| info.is_dir())
}

impl Scan {
    fn new(root: Str) -> Self {
        Self {
            stack: vec![root.clone()],
            entries: None,
            root,
        }
    }

    fn next(&mut self, follow: bool) -> Result<Option<(Str, bool)>> {
        loop {
            if let Some((dir, entries)) = &mut self.entries
                && let Some(name) = entries.next()
            {
                let path = join(dir, &string_name(name)?);
                let directory = is_dir(&path, follow);
                if directory {
                    self.stack.push(path.clone());
                }
                return Ok(Some((path, directory)));
            }
            let Some(dir) = self.stack.pop() else {
                return Ok(None);
            };
            let entries = snapshot(&dir, true)?.into_iter();
            self.entries = Some((dir, entries));
        }
    }

    fn relative<'a>(&self, path: &'a str) -> &'a str {
        if path == self.root.as_str() {
            ""
        } else if self.root == "." {
            path
        } else {
            path.strip_prefix(self.root.as_str())
                .unwrap()
                .trim_start_matches('/')
        }
    }
}

pub(super) struct Glob {
    parts: Vec<Part>,
    follow: bool,
    stack: Vec<Work>,
}

impl Glob {
    pub(super) fn new(
        root: Str,
        pattern: &str,
        case: usize,
        follow: bool,
        recursive: bool,
    ) -> Result<Self> {
        if pattern.starts_with('/') {
            return Err(Error::new(
                Kind::NotImplementedError,
                "Non-relative patterns are unsupported",
            ));
        }
        let mut input = Vec::new();
        if recursive {
            input.push("**");
        }
        input.extend(pattern.split('/').filter(|s| !s.is_empty() && *s != "."));
        if input.is_empty() {
            return Err(Error::new(
                Kind::ValueError,
                format!("Unacceptable pattern: '{pattern}'"),
            ));
        }
        if pattern.ends_with('/') || (recursive && pattern.is_empty()) {
            input.push("");
        }
        let sensitive = case != 1;
        let mut input = input.into_iter().peekable();
        let mut parts = Vec::new();
        while let Some(part) = input.next() {
            parts.push(match part {
                "" => Part::Directory,
                ".." => Part::Parent,
                "**" => {
                    while input.next_if(|s| *s == "**").is_some() {}
                    let mut tail = vec!["**"];
                    if follow {
                        while let Some(next) = input.next_if(|s| !matches!(*s, "" | "." | "..")) {
                            tail.push(next);
                        }
                    }
                    Part::Recursive(if tail.len() == 1 {
                        None
                    } else {
                        Some(glob_regex(&tail.join("/"), sensitive, true, '/')?)
                    })
                }
                literal if case == 0 && !literal.contains(['*', '?', '[']) => {
                    Part::Literal(literal.into())
                }
                pattern if sensitive => {
                    Part::Wild(NameMatch::Sensitive(Pattern::component(pattern)))
                }
                pattern => Part::Wild(NameMatch::Folded(glob_regex(pattern, false, false, '/')?)),
            });
        }
        Ok(Self {
            parts,
            follow,
            stack: vec![Work::Select {
                path: root,
                part: 0,
                known: false,
            }],
        })
    }

    pub(super) fn next(&mut self) -> Result<Option<Str>> {
        while let Some(work) = self.stack.pop() {
            match work {
                Work::Select { path, part, known } => {
                    let Some(selector) = self.parts.get(part) else {
                        if known || fs::symlink_metadata(path.as_str()).is_ok() {
                            return Ok(Some(path));
                        }
                        continue;
                    };
                    match selector {
                        Part::Literal(literal) => self.stack.push(Work::Select {
                            path: join(&path, literal),
                            part: part + 1,
                            known: false,
                        }),
                        Part::Parent => self.stack.push(Work::Select {
                            path: join(&path, ".."),
                            part: part + 1,
                            known,
                        }),
                        Part::Directory if known || is_dir(&path, true) => return Ok(Some(path)),
                        Part::Directory => (),
                        Part::Wild(_) => {
                            let names = snapshot(&path, true)?.into_iter();
                            self.stack.push(Work::Children { path, part, names });
                        }
                        Part::Recursive(filter) => {
                            self.stack.push(Work::Descendants {
                                part,
                                scan: Scan::new(path.clone()),
                            });
                            // ** can match zero components. Run the remaining selectors
                            // before scanning, even when this directory may not exist.
                            if filter.as_ref().is_none_or(|f| f.is_match("")) {
                                if part + 1 < self.parts.len() {
                                    self.stack.push(Work::Select {
                                        path,
                                        part: part + 1,
                                        known,
                                    });
                                } else if known || is_dir(&path, true) {
                                    return Ok(Some(path));
                                }
                            }
                        }
                    }
                }
                Work::Children {
                    path,
                    part,
                    mut names,
                } => {
                    let Part::Wild(pattern) = &self.parts[part] else {
                        unreachable!()
                    };
                    for name in names.by_ref() {
                        let name = string_name(name)?;
                        if !pattern.matches(&name) {
                            continue;
                        }
                        let child = join(&path, &name);
                        if part + 1 < self.parts.len() && !is_dir(&child, true) {
                            continue;
                        }
                        self.stack.push(Work::Children { path, part, names });
                        self.stack.push(Work::Select {
                            path: child,
                            part: part + 1,
                            known: true,
                        });
                        break;
                    }
                }
                Work::Descendants { part, mut scan } => {
                    let Part::Recursive(filter) = &self.parts[part] else {
                        unreachable!()
                    };
                    while let Some((path, dir)) = scan.next(self.follow)? {
                        if (part + 1 < self.parts.len() && !dir)
                            || filter
                                .as_ref()
                                .is_some_and(|f| !f.is_match(scan.relative(&path)))
                        {
                            continue;
                        }
                        self.stack.push(Work::Descendants { part, scan });
                        self.stack.push(Work::Select {
                            path,
                            part: part + 1,
                            known: true,
                        });
                        break;
                    }
                }
            }
        }
        Ok(None)
    }
}
