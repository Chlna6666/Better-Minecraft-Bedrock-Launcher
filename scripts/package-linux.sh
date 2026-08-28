#!/usr/bin/env bash
set -euo pipefail

APP_ID="io.github.chlna6666.BMCBL"
PACKAGE_NAME="bmcbl"
DISPLAY_NAME="BMCBL"
ARCH="x86_64"
DEB_ARCH="amd64"
FLATPAK_RUNTIME_VERSION="24.08"

VERSION=""
CHANNEL="stable"
BINARY="target/release/BMCBL"
OUTPUT_DIR="dist/linux"

usage() {
  cat <<USAGE
Usage: $0 --version VERSION [options]

Options:
  --channel stable|nightly   Build channel (default: stable)
  --binary PATH              Compiled BMCBL binary (default: target/release/BMCBL)
  --output-dir PATH          Output directory (default: dist/linux)
  -h, --help                 Show this help
USAGE
}

while (($#)); do
  case "$1" in
    --version)
      VERSION="${2:?missing value for --version}"
      shift 2
      ;;
    --channel)
      CHANNEL="${2:?missing value for --channel}"
      shift 2
      ;;
    --binary)
      BINARY="${2:?missing value for --binary}"
      shift 2
      ;;
    --output-dir)
      OUTPUT_DIR="${2:?missing value for --output-dir}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$VERSION" ]]; then
  echo "--version is required" >&2
  exit 2
fi
if [[ "$CHANNEL" != "stable" && "$CHANNEL" != "nightly" ]]; then
  echo "--channel must be stable or nightly" >&2
  exit 2
fi
if [[ ! -x "$BINARY" ]]; then
  echo "Compiled binary is missing or not executable: $BINARY" >&2
  exit 1
fi

for command in dpkg-deb rpmbuild curl file ldd flatpak flatpak-builder; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Required packaging command is unavailable: $command" >&2
    exit 1
  fi
done

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DESKTOP_SOURCE="$ROOT_DIR/packaging/linux/$APP_ID.desktop"
METAINFO_SOURCE="$ROOT_DIR/packaging/linux/$APP_ID.metainfo.xml"
ICON_SOURCE="$ROOT_DIR/assets/icons/icon.ico"

for source in "$DESKTOP_SOURCE" "$METAINFO_SOURCE" "$ICON_SOURCE"; do
  if [[ ! -f "$source" ]]; then
    echo "Required packaging source is missing: $source" >&2
    exit 1
  fi
done

OUTPUT_DIR="$(mkdir -p "$OUTPUT_DIR" && cd "$OUTPUT_DIR" && pwd)"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

ARTIFACT_VERSION="${VERSION#v}"
FILE_VERSION="${ARTIFACT_VERSION//\//-}"
DEB_VERSION="${ARTIFACT_VERSION/-nightly./~nightly.}"
DEB_VERSION="$(printf '%s' "$DEB_VERSION" | sed -E 's/[^0-9A-Za-z.+:~_-]+/./g')"
RPM_VERSION="$(printf '%s' "$ARTIFACT_VERSION" | sed -E 's/-.*$//; s/[^0-9A-Za-z._+]+/./g')"
if [[ "$CHANNEL" == "nightly" ]]; then
  RPM_RELEASE="0.$(printf '%s' "$ARTIFACT_VERSION" | sed -E 's/^[^-]+-//; s/[^0-9A-Za-z._+]+/./g')"
else
  RPM_RELEASE="1"
fi
BUILD_DATE="$(date -u +%F)"

convert_icon() {
  local destination="$1"
  if command -v magick >/dev/null 2>&1; then
    magick "${ICON_SOURCE}[0]" -background none -alpha on -resize 256x256 "$destination"
  elif command -v convert >/dev/null 2>&1; then
    convert "${ICON_SOURCE}[0]" -background none -alpha on -resize 256x256 "$destination"
  else
    echo "ImageMagick is required to convert assets/icons/icon.ico" >&2
    exit 1
  fi
}

install_payload() {
  local root="$1"
  install -Dm755 "$BINARY" "$root/usr/bin/BMCBL"
  install -Dm644 "$DESKTOP_SOURCE" "$root/usr/share/applications/$APP_ID.desktop"
  install -Dm644 "$METAINFO_SOURCE" "$root/usr/share/metainfo/$APP_ID.metainfo.xml"
  install -Dm644 "$ROOT_DIR/LICENSE" "$root/usr/share/licenses/$PACKAGE_NAME/LICENSE"
  install -d "$root/usr/share/icons/hicolor/256x256/apps"
  convert_icon "$root/usr/share/icons/hicolor/256x256/apps/$APP_ID.png"
}

APPDIR="$WORK_DIR/AppDir"
install_payload "$APPDIR"
cp "$APPDIR/usr/share/icons/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
cp "$APPDIR/usr/share/applications/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
cat > "$APPDIR/AppRun" <<'APP_RUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin${PATH:+:$PATH}"
export LD_LIBRARY_PATH="$HERE/usr/lib:$HERE/usr/lib64${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$HERE/usr/bin/BMCBL" "$@"
APP_RUN
chmod 0755 "$APPDIR/AppRun"

# Debian package
DEB_ROOT="$WORK_DIR/deb"
install_payload "$DEB_ROOT"
mkdir -p "$DEB_ROOT/DEBIAN"
cat > "$DEB_ROOT/DEBIAN/control" <<EOF_CONTROL
Package: $PACKAGE_NAME
Version: $DEB_VERSION
Section: games
Priority: optional
Architecture: $DEB_ARCH
Maintainer: Chlna6666
Homepage: https://www.bmcbl.com/
Depends: libasound2, libfontconfig1, libfreetype6, libgcc-s1, libssl3, libvulkan1, libwayland-client0, libx11-6, libxkbcommon0
Description: Better Minecraft Bedrock Launcher
 A graphical launcher and version manager for Minecraft Bedrock Edition.
EOF_CONTROL
DEB_OUTPUT="$OUTPUT_DIR/BMCBL-linux-$ARCH-v$FILE_VERSION.deb"
dpkg-deb --build --root-owner-group "$DEB_ROOT" "$DEB_OUTPUT"

# RPM package
RPM_TOP="$WORK_DIR/rpmbuild"
mkdir -p "$RPM_TOP"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
PAYLOAD_TAR="$RPM_TOP/SOURCES/bmcbl-payload.tar.gz"
tar -C "$APPDIR" -czf "$PAYLOAD_TAR" usr
cat > "$RPM_TOP/SPECS/bmcbl.spec" <<EOF_SPEC
Name:           $PACKAGE_NAME
Version:        $RPM_VERSION
Release:        $RPM_RELEASE%{?dist}
Summary:        Better Minecraft Bedrock Launcher
License:        GPL-3.0
URL:            https://www.bmcbl.com/
Source0:        bmcbl-payload.tar.gz
BuildArch:      x86_64
Requires:       alsa-lib, fontconfig, freetype, openssl-libs, vulkan-loader, libwayland-client, libX11, libxkbcommon

%description
A graphical launcher and version manager for Minecraft Bedrock Edition.

%prep
%setup -q -c -T
%{__tar} -xzf %{SOURCE0}

%install
rm -rf %{buildroot}
mkdir -p %{buildroot}
cp -a usr %{buildroot}/

%files
/usr/bin/BMCBL
/usr/share/applications/$APP_ID.desktop
/usr/share/icons/hicolor/256x256/apps/$APP_ID.png
/usr/share/metainfo/$APP_ID.metainfo.xml
/usr/share/licenses/$PACKAGE_NAME/LICENSE

%changelog
* $(LC_ALL=C date -u '+%a %b %d %Y') Chlna6666 - $RPM_VERSION-$RPM_RELEASE
- Automated $CHANNEL build.
EOF_SPEC
rpmbuild --define "_topdir $RPM_TOP" -bb "$RPM_TOP/SPECS/bmcbl.spec"
RPM_BUILT="$(find "$RPM_TOP/RPMS" -type f -name '*.rpm' -print -quit)"
if [[ -z "$RPM_BUILT" ]]; then
  echo "rpmbuild did not produce an RPM" >&2
  exit 1
fi
RPM_OUTPUT="$OUTPUT_DIR/BMCBL-linux-$ARCH-v$FILE_VERSION.rpm"
cp "$RPM_BUILT" "$RPM_OUTPUT"

# AppImage. linuxdeploy bundles non-base shared libraries from the host build.
LINUXDEPLOY="$WORK_DIR/linuxdeploy-$ARCH.AppImage"
curl --fail --location --retry 5 --retry-all-errors \
  "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$ARCH.AppImage" \
  --output "$LINUXDEPLOY"
chmod 0755 "$LINUXDEPLOY"
(
  cd "$WORK_DIR"
  ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$LINUXDEPLOY" \
    --appdir "$APPDIR" \
    --desktop-file "$DESKTOP_SOURCE" \
    --icon-file "$APPDIR/$APP_ID.png" \
    --output appimage
)
APPIMAGE_BUILT="$(find "$WORK_DIR" -maxdepth 1 -type f -name '*.AppImage' ! -name 'linuxdeploy-*' -print -quit)"
if [[ -z "$APPIMAGE_BUILT" ]]; then
  echo "linuxdeploy did not produce an AppImage" >&2
  exit 1
fi
APPIMAGE_OUTPUT="$OUTPUT_DIR/BMCBL-linux-$ARCH-v$FILE_VERSION.AppImage"
mv "$APPIMAGE_BUILT" "$APPIMAGE_OUTPUT"
chmod 0755 "$APPIMAGE_OUTPUT"

# Flatpak bundle. Reuse linuxdeploy's AppDir so non-runtime libraries are copied
# into /app/lib while the Flatpak runtime provides the system ABI.
FLATPAK_SOURCE="$WORK_DIR/flatpak-source"
mkdir -p "$FLATPAK_SOURCE"
cp -a "$APPDIR/usr" "$FLATPAK_SOURCE/usr"
cat > "$FLATPAK_SOURCE/BMCBL-flatpak" <<'FLATPAK_RUN'
#!/bin/sh
export LD_LIBRARY_PATH="/app/lib:/app/lib64${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec /app/bin/BMCBL "$@"
FLATPAK_RUN
chmod 0755 "$FLATPAK_SOURCE/BMCBL-flatpak"
cat > "$WORK_DIR/flatpak-manifest.yml" <<EOF_FLATPAK
app-id: $APP_ID
runtime: org.freedesktop.Platform
runtime-version: '$FLATPAK_RUNTIME_VERSION'
sdk: org.freedesktop.Sdk
command: BMCBL-flatpak
finish-args:
  - --share=network
  - --share=ipc
  - --socket=wayland
  - --socket=fallback-x11
  - --socket=pulseaudio
  - --device=dri
  - --filesystem=home
  - --talk-name=org.freedesktop.portal.Desktop
modules:
  - name: bmcbl
    buildsystem: simple
    build-commands:
      - install -Dm755 BMCBL-flatpak /app/bin/BMCBL-flatpak
      - cp -a usr/bin/BMCBL /app/bin/BMCBL
      - mkdir -p /app/lib /app/lib64 /app/share
      - if [ -d usr/lib ]; then cp -a usr/lib/. /app/lib/; fi
      - if [ -d usr/lib64 ]; then cp -a usr/lib64/. /app/lib64/; fi
      - cp -a usr/share/. /app/share/
    sources:
      - type: dir
        path: flatpak-source
EOF_FLATPAK
FLATPAK_REPO="$WORK_DIR/flatpak-repo"
FLATPAK_BUILD="$WORK_DIR/flatpak-build"
flatpak-builder --user --force-clean --repo="$FLATPAK_REPO" \
  "$FLATPAK_BUILD" "$WORK_DIR/flatpak-manifest.yml"
FLATPAK_OUTPUT="$OUTPUT_DIR/BMCBL-linux-$ARCH-v$FILE_VERSION.flatpak"
flatpak build-bundle "$FLATPAK_REPO" "$FLATPAK_OUTPUT" "$APP_ID" stable

sha256sum "$DEB_OUTPUT" "$RPM_OUTPUT" "$APPIMAGE_OUTPUT" "$FLATPAK_OUTPUT" \
  > "$OUTPUT_DIR/BMCBL-linux-$ARCH-v$FILE_VERSION.sha256"

printf 'Linux packages created:\n'
printf '  %s\n' "$DEB_OUTPUT" "$RPM_OUTPUT" "$APPIMAGE_OUTPUT" "$FLATPAK_OUTPUT"
