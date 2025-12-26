# Helper script to generate Langfuse authorization header for Codex config
# Usage: .\generate-langfuse-auth.ps1

Write-Host "==========================================" -ForegroundColor Cyan
Write-Host "Langfuse Authorization Header Generator" -ForegroundColor Cyan
Write-Host "==========================================" -ForegroundColor Cyan
Write-Host ""

# Prompt for API keys
Write-Host "Enter your Langfuse API keys (from https://cloud.langfuse.com):"
Write-Host ""

$PublicKey = Read-Host "Public Key (pk-lf-...)"
$SecretKey = Read-Host "Secret Key (sk-lf-...)"

Write-Host ""

# Validate keys are not empty
if ([string]::IsNullOrWhiteSpace($PublicKey) -or [string]::IsNullOrWhiteSpace($SecretKey)) {
    Write-Host "Error: Both keys are required." -ForegroundColor Red
    exit 1
}

# Validate key format
if (-not $PublicKey.StartsWith("pk-lf-")) {
    Write-Host "Warning: Public key should start with 'pk-lf-'" -ForegroundColor Yellow
}

if (-not $SecretKey.StartsWith("sk-lf-")) {
    Write-Host "Warning: Secret key should start with 'sk-lf-'" -ForegroundColor Yellow
}

# Generate base64 encoded auth string
$Credentials = "${PublicKey}:${SecretKey}"
$Bytes = [System.Text.Encoding]::UTF8.GetBytes($Credentials)
$AuthString = [Convert]::ToBase64String($Bytes)

Write-Host "==========================================" -ForegroundColor Cyan
Write-Host "Generated Authorization Header:" -ForegroundColor Cyan
Write-Host "==========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "`"Authorization`" = `"Basic $AuthString`"" -ForegroundColor Green
Write-Host ""

Write-Host "==========================================" -ForegroundColor Cyan
Write-Host "Add this to your ~/.codex/config.toml:" -ForegroundColor Cyan
Write-Host "==========================================" -ForegroundColor Cyan
Write-Host ""

$ConfigExample = @"
[otel]
environment = "production"
exporter = "otlp-http"
log_user_prompt = false

[otel.exporter."otlp-http"]
endpoint = "https://cloud.langfuse.com/api/public/otel"
protocol = "binary"

[otel.exporter."otlp-http".headers]
"Authorization" = "Basic $AuthString"
"@

Write-Host $ConfigExample -ForegroundColor White
Write-Host ""

Write-Host "==========================================" -ForegroundColor Cyan
Write-Host "Or use environment variable (more secure):" -ForegroundColor Cyan
Write-Host "==========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "`$env:LANGFUSE_AUTH = `"Basic $AuthString`"" -ForegroundColor Green
Write-Host ""
Write-Host "Then in config.toml:" -ForegroundColor White

$EnvConfigExample = @"

[otel.exporter."otlp-http".headers]
"Authorization" = "`${LANGFUSE_AUTH}"
"@

Write-Host $EnvConfigExample -ForegroundColor White
Write-Host ""

Write-Host "==========================================" -ForegroundColor Cyan
Write-Host "Done! 🎉" -ForegroundColor Green
Write-Host "==========================================" -ForegroundColor Cyan
