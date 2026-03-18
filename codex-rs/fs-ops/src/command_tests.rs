use super::FsCommand;
use super::parse_command_from_args;
use pretty_assertions::assert_eq;

#[test]
fn parse_read_bytes_command() {
    let command = parse_command_from_args(
        ["read_bytes", "/tmp/example.png"]
            .into_iter()
            .map(Into::into),
    )
    .expect("command should parse");

    assert_eq!(
        command,
        FsCommand::ReadBytes {
            path: "/tmp/example.png".into(),
        }
    );
}
