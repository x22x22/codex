use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = env::args_os();
    let _program = args.next();
    let output_path = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow::anyhow!("expected output path as first argument"))?,
    );
    let payload = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("expected payload as final argument"))?;

    let mut file = File::create(&output_path)?;
    file.write_all(payload.to_string_lossy().as_bytes())?;
    file.sync_all()?;

    Ok(())
}
