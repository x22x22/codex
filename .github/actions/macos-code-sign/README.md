# macos-code-sign

This action can sign the default Rust release binaries or an explicit list of macOS code paths.

By default, alpha and beta tag builds automatically receive the shared keychain entitlement plist.
Set `shared-keychain-entitlement: false` to opt out, or `true` to force it on for other builds.

To sign a `Codex.app` bundle with the shared keychain entitlement, pass the app path plus the
checked-in entitlements plist:

```yaml
- uses: ./.github/actions/macos-code-sign
  with:
    target: aarch64-apple-darwin
    sign-binaries: "true"
    sign-dmg: "false"
    sign-paths: |
      path/to/Codex.app
    shared-keychain-entitlement: true
```

If only some signed paths should receive the entitlements plist, set `entitlements-paths` to the
subset of paths that should be signed with `--entitlements`. You can still provide
`entitlements-file` directly if a workflow needs a different plist.
