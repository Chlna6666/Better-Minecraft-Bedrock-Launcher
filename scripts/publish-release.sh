#!/usr/bin/env bash
set -euo pipefail

asset_dir="${1:?asset directory is required}"
mapfile -t assets < <(find "$asset_dir" -maxdepth 1 -type f -print | sort)
if ((${#assets[@]} == 0)); then
  echo "No release assets were downloaded" >&2
  exit 1
fi

notes="$RUNNER_TEMP/release-notes.md"
if [[ "$RELEASE_CHANNEL" == "nightly" ]]; then
  cat > "$notes" <<'EOF'
This is an automated nightly pre-release build for Windows and Linux.

Linux artifacts include DEB, RPM, AppImage and Flatpak packages. Nightly builds may be unstable and are intended for testing.

## Changes
EOF
else
  cat > "$notes" <<'EOF'
Stable BMCBL release for Windows and Linux.

Linux artifacts include DEB, RPM, AppImage and Flatpak packages.

## Changes
EOF
fi

append_changes() {
  git log --no-merges --pretty='%s%x09%h' "$@" \
    | awk -F '\t' 'tolower($1) !~ /^ci(\([^)]*\))?!?:[[:space:]]/ { printf "- %s (%s)\n", $1, $2 }' \
    >> "$notes"
}

previous="$(git tag --list 'v*' --sort=-version:refname | grep -Ev 'nightly' | grep -Fxv "$RELEASE_TAG" | head -n 1 || true)"
if [[ -n "$previous" ]]; then
  append_changes "$previous..$RELEASE_REF"
else
  append_changes -n 100 "$RELEASE_REF"
fi

if gh release view "$RELEASE_TAG" --repo "$GITHUB_REPOSITORY" >/dev/null 2>&1; then
  gh release upload "$RELEASE_TAG" "${assets[@]}" --repo "$GITHUB_REPOSITORY" --clobber
  edit_args=("$RELEASE_TAG" --repo "$GITHUB_REPOSITORY" --title "$RELEASE_TITLE" --notes-file "$notes")
  if [[ "$RELEASE_PRERELEASE" == "true" ]]; then
    edit_args+=(--prerelease)
  else
    edit_args+=(--latest)
  fi
  gh release edit "${edit_args[@]}"
else
  create_args=("$RELEASE_TAG" "${assets[@]}" --repo "$GITHUB_REPOSITORY" --title "$RELEASE_TITLE" --notes-file "$notes" --target "$RELEASE_REF")
  if [[ "$RELEASE_PRERELEASE" == "true" ]]; then
    create_args+=(--prerelease)
  else
    create_args+=(--verify-tag --latest)
  fi
  gh release create "${create_args[@]}"
fi

if [[ "$CLEANUP_OLD_NIGHTLIES" == "true" ]]; then
  mapfile -t old_tags < <(
    gh release list --repo "$GITHUB_REPOSITORY" --limit 100 \
      --json tagName,isPrerelease \
      --jq '.[] | select(.isPrerelease == true) | .tagName' \
      | grep -- '-nightly\.' | grep -Fxv "$RELEASE_TAG" || true
  )
  for tag in "${old_tags[@]}"; do
    gh release delete "$tag" --repo "$GITHUB_REPOSITORY" --yes --cleanup-tag || \
      git push origin --delete "$tag" || true
  done
fi
