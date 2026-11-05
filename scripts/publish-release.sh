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

stable_base="$(git tag --list 'v*' --sort=-version:refname | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | grep -Fxv "$RELEASE_TAG" | head -n 1 || true)"
previous="$stable_base"

# Previous nightly releases and their tags are deleted by cleanup. A separate
# moving baseline tag preserves the actual last-published commit between runs.
if [[ "$RELEASE_CHANNEL" == "nightly" ]]; then
  if git ls-remote --exit-code --refs origin refs/tags/bmcbl-nightly-baseline >/dev/null 2>&1; then
    git fetch --force origin '+refs/tags/bmcbl-nightly-baseline:refs/tags/bmcbl-nightly-baseline'
    previous="bmcbl-nightly-baseline"
  else
    previous="$(git tag --list 'v*-nightly.*' --sort=-creatordate | grep -Fxv "$RELEASE_TAG" | head -n 1 || true)"
    if [[ -z "$previous" ]]; then previous="$stable_base"; fi
  fi
fi

if [[ -n "$previous" ]]; then
  python3 scripts/release_notes.py --format release --since "$previous" --until "$RELEASE_REF" --repo "$GITHUB_REPOSITORY" >> "$notes"
else
  python3 scripts/release_notes.py --format release --until "$RELEASE_REF" --repo "$GITHUB_REPOSITORY" >> "$notes"
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
  create_args=("$RELEASE_TAG" "${assets[@]}" --repo "$GITHUB_REPOSITORY" --title "$RELEASE_TITLE" --notes-file "$notes")
  if [[ "$RELEASE_PRERELEASE" == "true" ]]; then
    target_commit="$(git rev-parse "${RELEASE_REF}^{commit}" 2>/dev/null || git rev-parse "$RELEASE_REF" 2>/dev/null || echo "$RELEASE_REF")"
    create_args+=(--target "$target_commit" --prerelease)
  else
    create_args+=(--verify-tag --latest)
  fi
  gh release create "${create_args[@]}"
fi

# Advance only after publishing successfully; the tag is not a v* release tag.
if [[ "$RELEASE_CHANNEL" == "nightly" ]]; then
  git tag -f bmcbl-nightly-baseline "$RELEASE_REF"
  git push --force origin refs/tags/bmcbl-nightly-baseline
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
