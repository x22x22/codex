#![warn(argument_comment_mismatch)]

fn legacy_create_openai_url(base_url: Option<String>) -> String {
    let _ = base_url;
    String::new()
}

fn main() {
    let _ = legacy_create_openai_url(/*api_base=*/ None);
}
