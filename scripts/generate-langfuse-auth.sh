#!/usr/bin/env bash
# Helper script to generate Langfuse authorization header for Codex config
# Usage: ./generate-langfuse-auth.sh

set -e

echo "=========================================="
echo "Langfuse Authorization Header Generator"
echo "=========================================="
echo ""

# Check if base64 command is available
if ! command -v base64 &> /dev/null; then
    echo "Error: 'base64' command not found. Please install coreutils."
    exit 1
fi

# Prompt for API keys
echo "Enter your Langfuse API keys (from https://cloud.langfuse.com):"
echo ""

read -p "Public Key (pk-lf-...): " PUBLIC_KEY
read -p "Secret Key (sk-lf-...): " SECRET_KEY

echo ""

# Validate keys are not empty
if [ -z "$PUBLIC_KEY" ] || [ -z "$SECRET_KEY" ]; then
    echo "Error: Both keys are required."
    exit 1
fi

# Validate key format
if [[ ! "$PUBLIC_KEY" =~ ^pk-lf- ]]; then
    echo "Warning: Public key should start with 'pk-lf-'"
fi

if [[ ! "$SECRET_KEY" =~ ^sk-lf- ]]; then
    echo "Warning: Secret key should start with 'sk-lf-'"
fi

# Generate base64 encoded auth string
AUTH_STRING=$(echo -n "${PUBLIC_KEY}:${SECRET_KEY}" | base64)

# Remove newlines if any (for GNU base64)
AUTH_STRING=$(echo "$AUTH_STRING" | tr -d '\n')

echo "=========================================="
echo "Generated Authorization Header:"
echo "=========================================="
echo ""
echo "\"Authorization\" = \"Basic ${AUTH_STRING}\""
echo ""
echo "=========================================="
echo "Add this to your ~/.codex/config.toml:"
echo "=========================================="
echo ""
cat << EOF
[otel]
environment = "production"
exporter = "otlp-http"
log_user_prompt = false

[otel.exporter."otlp-http"]
endpoint = "https://cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic ${AUTH_STRING}"
EOF
echo ""
echo "=========================================="
echo "Or use environment variable (more secure):"
echo "=========================================="
echo ""
echo "export LANGFUSE_AUTH=\"Basic ${AUTH_STRING}\""
echo ""
echo "Then in config.toml:"
cat << EOF

[otel.exporter."otlp-http".headers]
"Authorization" = "\${LANGFUSE_AUTH}"
EOF
echo ""
echo "=========================================="
echo "Done! 🎉"
echo "=========================================="
