#[tokio::main]
async fn main() {
    if let Err(err) = codex_login::run_api_provision_helper_from_env().await {
        eprintln!("{err}");
        if let Some(body) = err.body()
            && !body.is_empty()
        {
            eprintln!("{body}");
        }
        std::process::exit(1);
    }
}
