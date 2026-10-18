//! Framed-request entrypoint sharing the unchanged incremental graph algorithm.

#[path = "main.rs"]
#[allow(dead_code)]
mod planner;

fn main() -> std::process::ExitCode {
    planner::batch_main()
}
