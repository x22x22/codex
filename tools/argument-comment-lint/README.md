# argument-comment-lint

Isolated [Dylint](https://github.com/trailofbits/dylint) library for enforcing
Rust argument comments in the exact `/*param=*/` shape.

It provides two lints:

- `argument_comment_mismatch` (`warn` by default): validates that a present
  `/*param=*/` comment matches the resolved callee parameter name.
- `uncommented_anonymous_literal_argument` (`allow` by default): flags
  anonymous literal-like arguments such as `None`, `true`, `false`, and numeric
  literals when they do not have a preceding `/*param=*/` comment.

String and char literals are exempt because they are often already
self-descriptive at the callsite.

## Behavior

When you own the API, prefer a clearer shape over positional literal arguments:

```rust
enum BaseUrl {
    Default,
    Custom(String),
}

struct RetryCount(usize);

fn create_openai_url(base_url: BaseUrl, retry_count: RetryCount) -> String {
    let _ = (base_url, retry_count);
    String::new()
}
```

```rust
create_openai_url(BaseUrl::Default, RetryCount(3));
```

When a minimal refactor needs to keep a legacy signature, `/*param=*/` comments
make those call sites readable:

```rust
fn legacy_create_openai_url(base_url: Option<String>, retry_count: usize) -> String {
    let _ = (base_url, retry_count);
    String::new()
}
```

```rust
legacy_create_openai_url(/*base_url=*/ None, /*retry_count=*/ 3);
```

This is warned on by `argument_comment_mismatch`:

```rust
legacy_create_openai_url(/*api_base=*/ None, 3);
```

This is only warned on when `uncommented_anonymous_literal_argument` is enabled:

```rust
legacy_create_openai_url(None, 3);
```

## Development

Install the required tooling once:

```bash
cargo install cargo-dylint dylint-link
rustup toolchain install nightly-2025-09-18 \
  --component llvm-tools-preview \
  --component rustc-dev \
  --component rust-src
```

Run the lint crate tests:

```bash
cd tools/argument-comment-lint
cargo test
```

Run the lint against `codex-rs` from the repo root:

```bash
./tools/argument-comment-lint/run.sh -p codex-core
just argument-comment-lint -p codex-core
```

If no package selection is provided, `run.sh` defaults to checking the
`codex-rs` workspace with `--workspace --no-deps`.

Repo runs also promote `uncommented_anonymous_literal_argument` to an error by
default:

```bash
./tools/argument-comment-lint/run.sh -p codex-core
```

The wrapper does that by setting `DYLINT_RUSTFLAGS`, and it leaves an explicit
existing setting alone. To override that behavior for an ad hoc run:

```bash
DYLINT_RUSTFLAGS="-A uncommented_anonymous_literal_argument" \
  ./tools/argument-comment-lint/run.sh -p codex-core
```

To expand target coverage for an ad hoc run:

```bash
./tools/argument-comment-lint/run.sh -p codex-core -- --all-targets
```
