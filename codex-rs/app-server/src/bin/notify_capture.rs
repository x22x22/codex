use std::env;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;

fn append_log(log_path: &Path, message: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(file, "{message}");
        let _ = file.sync_all();
    }
}

fn main() -> Result<()> {
    let mut args = env::args_os();
    let _program = args.next();
    let output_path = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow!("expected output path as first argument"))?,
    );
    let log_path = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow!("expected log path as second argument"))?,
    );
    let payload = args
        .next()
        .ok_or_else(|| anyhow!("expected payload as final argument"))?;

    append_log(
        &log_path,
        &format!(
            "start cwd={} output={}",
            env::current_dir()?.display(),
            output_path.display()
        ),
    );

    if args.next().is_some() {
        append_log(&log_path, "unexpected extra argument");
        bail!("expected payload as final argument");
    }

    let payload = payload.to_string_lossy();
    append_log(&log_path, &format!("payload-bytes={}", payload.len()));

    let mut file = File::create(&output_path)
        .with_context(|| format!("failed to create {}", output_path.display()))?;
    file.write_all(payload.as_bytes())
        .with_context(|| format!("failed to write {}", output_path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync {}", output_path.display()))?;

    append_log(&log_path, &format!("wrote {}", output_path.display()));
    Ok(())
}
