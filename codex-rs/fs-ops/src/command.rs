use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsCommand {
    ReadBytes { path: PathBuf },
    ReadText { path: PathBuf },
}

pub fn parse_command_from_args(
    mut args: impl Iterator<Item = OsString>,
) -> Result<FsCommand, String> {
    let Some(operation) = args.next() else {
        return Err("missing operation".to_string());
    };
    let Some(operation) = operation.to_str() else {
        return Err("operation must be valid UTF-8".to_string());
    };
    let Some(path) = args.next() else {
        return Err(format!("missing path for operation `{operation}`"));
    };
    if args.next().is_some() {
        return Err(format!(
            "unexpected extra arguments for operation `{operation}`"
        ));
    }

    let path = PathBuf::from(path);
    match operation {
        "read_bytes" => Ok(FsCommand::ReadBytes { path }),
        "read_text" => Ok(FsCommand::ReadText { path }),
        _ => Err(format!(
            "unsupported filesystem operation `{operation}`; expected one of `read_bytes`, `read_text`"
        )),
    }
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
