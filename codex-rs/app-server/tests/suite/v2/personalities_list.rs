use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use app_test_support::McpProcess;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::PersonalitiesListParams;
use codex_app_server_protocol::PersonalitiesListResponse;
use codex_app_server_protocol::PersonalityScope;
use codex_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

fn write_personality(
    root: &std::path::Path,
    name: &str,
    description: &str,
    body: &str,
) -> Result<()> {
    std::fs::create_dir_all(root)?;
    std::fs::write(
        root.join(format!("{name}.md")),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"),
    )?;
    Ok(())
}

#[tokio::test]
async fn personalities_list_returns_builtin_and_file_backed_personalities() -> Result<()> {
    let codex_home = TempDir::new()?;
    let repo = TempDir::new()?;
    let repo_personalities = repo.path().join(".codex/personalities");
    let user_personalities = codex_home.path().join("personalities");

    write_personality(
        &user_personalities,
        "night-owl",
        "User personality",
        "User instructions",
    )?;
    write_personality(
        &repo_personalities,
        "night-owl",
        "Repo personality",
        "Repo instructions",
    )?;
    write_personality(
        &repo_personalities,
        "ship-it",
        "Repo only",
        "Ship it instructions",
    )?;
    std::fs::write(
        repo_personalities.join("broken.md"),
        "# missing frontmatter\n",
    )?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_personalities_list_request(PersonalitiesListParams {
            cwds: Some(vec![repo.path().to_path_buf().try_into()?]),
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let PersonalitiesListResponse { data } = to_response(response)?;

    assert_eq!(data.len(), 1);
    assert_eq!(data[0].cwd, repo.path().to_path_buf());

    let personalities = &data[0].personalities;
    let names = personalities
        .iter()
        .map(|personality| personality.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["friendly", "none", "pragmatic", "night-owl", "ship-it"]
    );
    assert!(personalities.iter().any(|personality| {
        personality.name == "friendly"
            && personality.is_built_in
            && personality.scope == PersonalityScope::Builtin
    }));
    assert!(personalities.iter().any(|personality| {
        personality.name == "pragmatic"
            && personality.is_built_in
            && personality.scope == PersonalityScope::Builtin
    }));
    assert!(personalities.iter().any(|personality| {
        personality.name == "none"
            && personality.is_built_in
            && personality.scope == PersonalityScope::Builtin
    }));

    let night_owl = personalities
        .iter()
        .find(|personality| personality.name == "night-owl")
        .ok_or_else(|| anyhow!("night-owl personality missing"))?;
    assert_eq!(night_owl.description, "Repo personality");
    assert_eq!(night_owl.scope, PersonalityScope::Repo);
    assert!(!night_owl.is_built_in);

    let ship_it = personalities
        .iter()
        .find(|personality| personality.name == "ship-it")
        .ok_or_else(|| anyhow!("ship-it personality missing"))?;
    assert_eq!(ship_it.description, "Repo only");
    assert_eq!(ship_it.scope, PersonalityScope::Repo);
    assert!(!ship_it.is_built_in);

    assert!(
        personalities
            .iter()
            .all(|personality| personality.name != "broken")
    );
    assert_eq!(
        personalities
            .iter()
            .filter(|personality| personality.name == "night-owl")
            .count(),
        1
    );
    Ok(())
}
