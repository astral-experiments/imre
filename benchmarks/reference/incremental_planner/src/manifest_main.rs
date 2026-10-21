//! File-manifest entrypoint sharing the unchanged batch parser and graph algorithm.

#[path = "main.rs"]
#[allow(dead_code)]
mod planner;

fn main() -> std::process::ExitCode {
    planner::manifest_main()
}
