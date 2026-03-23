// Aggregates all former standalone integration tests as modules.
use codex_arg0::Arg0PathEntryGuard;
use codex_arg0::arg0_dispatch;
use ctor::ctor;
use tempfile::TempDir;

struct TestCodexAliasesGuard {
    _codex_home: TempDir,
    _arg0: Arg0PathEntryGuard,
}

const CODEX_HOME_ENV_VAR: &str = "CODEX_HOME";

// This code runs before any other tests are run.
// It allows the test binary to behave like codex-linux-sandbox based on arg0.
#[ctor]
pub static CODEX_ALIASES_TEMP_DIR: TestCodexAliasesGuard = unsafe {
    #[allow(clippy::unwrap_used)]
    let codex_home = tempfile::Builder::new()
        .prefix("codex-linux-sandbox-tests")
        .tempdir()
        .unwrap();
    let previous_codex_home = std::env::var_os(CODEX_HOME_ENV_VAR);

    // Safety: #[ctor] runs before test threads start.
    unsafe {
        std::env::set_var(CODEX_HOME_ENV_VAR, codex_home.path());
    }
    #[allow(clippy::unwrap_used)]
    let arg0 = arg0_dispatch().unwrap();
    match previous_codex_home.as_ref() {
        Some(value) => unsafe {
            std::env::set_var(CODEX_HOME_ENV_VAR, value);
        },
        None => unsafe {
            std::env::remove_var(CODEX_HOME_ENV_VAR);
        },
    }

    TestCodexAliasesGuard {
        _codex_home: codex_home,
        _arg0: arg0,
    }
};

mod landlock;
mod managed_proxy;
