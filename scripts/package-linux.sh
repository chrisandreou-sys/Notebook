#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:-0.1.0}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
BIN="$ROOT/target/release/notebook-rs"
if [[ ! -x "$BIN" ]]; then echo "Build with cargo build --release first" >&2; exit 1; fi
mkdir -p dist
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Debian package
DEB="$WORK/deb"
install -Dm755 "$BIN" "$DEB/usr/bin/notebook-rs"
install -Dm644 assets/notebook-rs.svg "$DEB/usr/share/icons/hicolor/scalable/apps/notebook-rs.svg"
install -Dm644 assets/FONT-LICENSE.txt "$DEB/usr/share/doc/notebook-rs/font-copyright"
cat > "$WORK/notebook-rs.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Notebook RS
Comment=Source-grounded research and AI studio
Exec=notebook-rs
Icon=notebook-rs
Terminal=false
Categories=Education;Office;
DESKTOP
install -Dm644 "$WORK/notebook-rs.desktop" "$DEB/usr/share/applications/notebook-rs.desktop"
mkdir -p "$DEB/DEBIAN"
cat > "$DEB/DEBIAN/control" <<CONTROL
Package: notebook-rs
Version: $VERSION
Section: education
Priority: optional
Architecture: amd64
Maintainer: Notebook RS contributors <noreply@github.com>
Depends: libc6 (>= 2.35)
Description: Local-first research notebook and AI studio
 Upload and question sources, and create media using your own AI provider.
CONTROL
chmod 0644 "$DEB/DEBIAN/control"
dpkg-deb --build --root-owner-group "$DEB" "dist/notebook-rs_${VERSION}_amd64.deb"

# Self-contained AppImage, using the official appimagetool from GitHub.
APPDIR="$WORK/NotebookRS.AppDir"
install -Dm755 "$BIN" "$APPDIR/usr/bin/notebook-rs"
install -Dm644 assets/notebook-rs.svg "$APPDIR/usr/share/icons/hicolor/scalable/apps/notebook-rs.svg"
install -Dm644 "$WORK/notebook-rs.desktop" "$APPDIR/usr/share/applications/notebook-rs.desktop"
ln -s usr/bin/notebook-rs "$APPDIR/AppRun"
ln -s usr/share/applications/notebook-rs.desktop "$APPDIR/notebook-rs.desktop"
ln -s usr/share/icons/hicolor/scalable/apps/notebook-rs.svg "$APPDIR/notebook-rs.svg"
ln -s notebook-rs.svg "$APPDIR/.DirIcon"
TOOL="$WORK/appimagetool.AppImage"
curl --fail --location --retry 3 --silent --show-error \
  'https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage' -o "$TOOL"
chmod +x "$TOOL"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" "$APPDIR" "dist/NotebookRS_${VERSION}_x86_64.AppImage"
chmod +x "dist/NotebookRS_${VERSION}_x86_64.AppImage"
echo "Packages:"; ls -lh dist
