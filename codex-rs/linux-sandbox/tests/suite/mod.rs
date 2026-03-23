// Aggregates all former standalone integration tests as modules.
use ctor::ctor;
use std::path::Path;

const LINUX_SANDBOX_ARG0: &str = "codex-linux-sandbox";

// This code runs before any other tests are run.
// It allows the test binary to behave like codex-linux-sandbox when re-execed
// via current_exe() with argv[0] overridden.
#[ctor]
fn dispatch_linux_sandbox_arg0() {
    let argv0 = std::env::args_os().next().unwrap_or_default();
    let exe_name = Path::new(&argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    if exe_name == LINUX_SANDBOX_ARG0 {
        codex_linux_sandbox::run_main();
    }
}

mod landlock;
mod managed_proxy;
