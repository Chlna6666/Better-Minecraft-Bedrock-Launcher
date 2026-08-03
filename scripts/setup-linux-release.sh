#!/usr/bin/env bash
set -euo pipefail

sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  appstream build-essential clang cmake curl desktop-file-utils file \
  flatpak flatpak-builder imagemagick libasound2-dev libdbus-1-dev \
  libegl1-mesa-dev libfontconfig1-dev libfreetype6-dev libgl1-mesa-dev \
  libpipewire-0.3-dev libssl-dev libudev-dev libvulkan-dev libwayland-dev \
  libx11-dev libx11-xcb-dev libxcb-render0-dev libxcb-shape0-dev \
  libxcb-xfixes0-dev libxcb1-dev libxcursor-dev libxi-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libxrandr-dev llvm-dev nasm \
  ninja-build patchelf pkg-config rpm yasm

flatpak remote-add --user --if-not-exists flathub \
  https://flathub.org/repo/flathub.flatpakrepo
flatpak install --user --noninteractive -y flathub \
  org.freedesktop.Platform//24.08 org.freedesktop.Sdk//24.08

# package-linux.sh exports the bundle from the stable branch. flatpak-builder
# defaults to master unless a branch is supplied, so inject it consistently.
mkdir -p "$HOME/.local/bin"
cat > "$HOME/.local/bin/flatpak-builder" <<'WRAPPER'
#!/usr/bin/env bash
exec /usr/bin/flatpak-builder --default-branch=stable "$@"
WRAPPER
chmod 0755 "$HOME/.local/bin/flatpak-builder"
if [[ -n "${GITHUB_PATH:-}" ]]; then
  echo "$HOME/.local/bin" >> "$GITHUB_PATH"
fi
