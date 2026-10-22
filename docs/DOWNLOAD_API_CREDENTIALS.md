# Download API credentials

BMCBL supports two optional credentials from **Settings → Launcher → Download**.

## CurseForge API key

Use an optional personal CurseForge API key in `settings.toml`:

```toml
[launcher.download]
curseforge_api_key = "YOUR_KEY"
```

If unset, BMCBL uses the built-in key for official API calls. A personally supplied
key is sent to **https://api.curseforge.com** and, if an explicit custom HTTPS API
source is selected, that custom destination. Personal keys are **not** sent to
the automatically selected MCIM mirror; the mirror continues using the bundled
key if needed. HTTP custom endpoints never receive API key headers.

Key rotation changes CurseForge's response cache partition. Changes made in
Settings invalidate the visible resource results so a new client can be created.

## GitHub API personal access token

```toml
[launcher.download.github]
api_token = "YOUR_GITHUB_PAT"
```

This token is only attached as a Bearer token to the exact
`https://api.github.com` origin, including launcher updater release checks,
native mod release metadata, and Proton-GDK release metadata.
**No GitHub personal access token is sent to download mirrors.**
A token is not needed for public file downloads.

## Security and UI

Credential editors never prefill saved credentials or display them again.
They show whether a key is configured; paste a replacement and press Enter or
move focus to save, or use the explicit Clear action. Leaving the editor blank
retains the current credential.

Current `settings.toml` credential storage is **plain text**. Protect its
filesystem permissions, do not commit it to source control, and do not upload
it in bug reports. Configuration `Debug` rendering masks the key fields.
This is not equivalent to encrypted OS keyring storage.
