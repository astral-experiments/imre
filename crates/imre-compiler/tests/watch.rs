use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

struct Watch {
    child: Child,
    results: Receiver<String>,
}

impl Watch {
    fn start(flag: &str, entry: &Path, root: Option<&Path>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_imre"));
        command.arg("check").arg(flag).arg(entry);
        if let Some(root) = root {
            command.arg("--module-root").arg(root);
        }
        let mut child = command.stderr(Stdio::piped()).spawn().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (send, results) = mpsc::channel();
        std::thread::spawn(move || {
            let mut result = String::new();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                result.push_str(&line);
                result.push('\n');
                if matches!(line.as_str(), "Check passed" | "Check failed")
                    && send.send(std::mem::take(&mut result)).is_err()
                {
                    break;
                }
            }
        });
        Self { child, results }
    }

    fn expect(&self, entry: &Path, root: Option<&Path>, success: bool) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_imre"));
        command.arg("check").arg(entry);
        if let Some(root) = root {
            command.arg("--module-root").arg(root);
        }
        let ordinary = command.output().unwrap();
        assert!(ordinary.stdout.is_empty());
        assert_eq!(ordinary.status.success(), success);
        let mut expected = String::from_utf8(ordinary.stderr).unwrap();
        expected.push_str(if ordinary.status.success() {
            "Check passed\n"
        } else {
            "Check failed\n"
        });
        assert_eq!(
            self.results.recv_timeout(Duration::from_secs(15)).unwrap(),
            expected
        );
    }

    fn quiet(&self) {
        assert_eq!(
            self.results.recv_timeout(Duration::from_millis(150)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn watch_checks_object_fallback_after_type_inference() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.py");
    let helper = dir.path().join("helper.py");
    fs::write(&entry, "import helper\n").unwrap();
    // The list element hint keeps the incremental type database alive.
    let valid = "import ast\nast.parse('1')\ndef keep(flag: bool) -> int:\n    if flag:\n        values = []\n    else:\n        values = [1]\n    return len(values)\nkeep(True)\n";
    fs::write(&helper, valid).unwrap();
    let watch = Watch::start("--watch", &entry, None);
    watch.expect(&entry, None, true);
    fs::write(
        &helper,
        valid.replace("return len(values)", "return 'wrong'"),
    )
    .unwrap();
    watch.expect(&entry, None, false);
    fs::write(&helper, valid).unwrap();
    watch.expect(&entry, None, true);
}

#[test]
fn watch_recovers_from_missing_files_and_package_shadowing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let entry = root.join("main.py");
    let dep = root.join("dep.py");
    let package = root.join("dep");
    let initializer = package.join("__init__.py");
    let good = "def score() -> int:\n    return 2\n";
    let bad = "def score() -> int:\n    return 'two'\n";
    let watch = Watch::start("--watch", &entry, Some(root));
    let expect = |success| watch.expect(&entry, Some(root), success);
    expect(false);
    fs::write(&entry, "from dep import score\nprint(score())\n").unwrap();
    expect(false);
    fs::write(&dep, good).unwrap();
    expect(true);

    // Atomic saves with the same bytes and writes to non-inputs stay quiet.
    let saved = root.join("editor.tmp");
    fs::write(&saved, good).unwrap();
    fs::rename(&saved, &dep).unwrap();
    fs::write(root.join("another.py"), bad).unwrap();
    watch.quiet();

    fs::write(&saved, bad).unwrap();
    fs::rename(&saved, &dep).unwrap();
    expect(false);
    fs::create_dir(&package).unwrap();
    watch.quiet();
    fs::write(&initializer, good).unwrap();
    expect(true);
    fs::remove_file(&initializer).unwrap();
    expect(false);
    fs::remove_file(&dep).unwrap();
    expect(false);
    fs::write(&dep, good).unwrap();
    expect(true);

    fs::write(&entry, "print(1)\n").unwrap();
    expect(true);
    fs::write(&dep, bad).unwrap();
    watch.quiet();
}

#[test]
fn watch_reinfers_the_root_when_a_package_appears() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("pkg");
    fs::create_dir(&package).unwrap();
    let entry = package.join("main.py");
    fs::write(&entry, "from .dep import score\nprint(score())\n").unwrap();
    fs::write(
        package.join("dep.py"),
        "def score() -> int:\n    return 1\n",
    )
    .unwrap();
    let watch = Watch::start("-w", &entry, None);
    watch.expect(&entry, None, false);
    let initializer = package.join("__init__.py");
    fs::write(&initializer, "").unwrap();
    watch.expect(&entry, None, true);
    fs::remove_file(initializer).unwrap();
    watch.expect(&entry, None, false);
}

#[test]
#[cfg(unix)]
fn watch_follows_the_current_target_of_a_module_symlink() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let entry = root.join("main.py");
    let sources = root.join("sources");
    fs::create_dir(&sources).unwrap();
    let good = "def score() -> int:\n    return 1\n";
    fs::write(&entry, "from dep import score\nprint(score())\n").unwrap();
    fs::write(sources.join("one.py"), good).unwrap();
    fs::write(
        sources.join("two.py"),
        "def score() -> int:\n    return 'x'\n",
    )
    .unwrap();
    symlink("sources/one.py", root.join("dep.py")).unwrap();
    let watch = Watch::start("--watch", &entry, Some(root));
    watch.expect(&entry, Some(root), true);
    symlink("sources/two.py", root.join("saved")).unwrap();
    fs::rename(root.join("saved"), root.join("dep.py")).unwrap();
    watch.expect(&entry, Some(root), false);
    fs::write(sources.join("two.py"), good).unwrap();
    watch.expect(&entry, Some(root), true);
}
