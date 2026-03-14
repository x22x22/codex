#![warn(uncommented_anonymous_literal_argument)]

struct Client;

impl Client {
    fn set_legacy_flag(&self, enabled: bool) {}
}

fn legacy_create_openai_url(base_url: Option<String>, retry_count: usize) -> String {
    let _ = (base_url, retry_count);
    String::new()
}

fn main() {
    let client = Client;
    let _ = legacy_create_openai_url(None, 3);
    client.set_legacy_flag(true);
}
