#!/bin/sh
set -eu

REPO="team-loco/flock-app"
APP="Flock.app"

die() {
  printf 'flock install: %s\n' "$*" >&2
  exit 1
}

[ "$(uname -s)" = "Darwin" ] || die "Flock runs on macOS only"
[ "$(uname -m)" = "arm64" ] || die "Flock supports Apple Silicon (arm64) only"

if [ -n "${FLOCK_VERSION:-}" ]; then
  version="${FLOCK_VERSION#v}"
else
  tag="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
    sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
  [ -n "$tag" ] || die "could not resolve the latest release"
  version="${tag#v}"
fi

install_dir="${FLOCK_INSTALL_DIR:-/Applications}"
case "${FLOCK_LOCO:-0}" in
  1) zip="Flock-$version-loco-arm64.zip" ;;
  0) zip="Flock-$version-arm64.zip" ;;
  *) die "FLOCK_LOCO must be 0 or 1" ;;
esac
base="https://github.com/$REPO/releases/download/v$version"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT INT TERM

curl -fsSL -o "$work/$zip" "$base/$zip" || die "could not download $zip"
curl -fsSL -o "$work/$zip.sha256" "$base/$zip.sha256" || die "could not download $zip.sha256"
(cd "$work" && shasum -a 256 -c "$zip.sha256" >/dev/null) || die "checksum mismatch for $zip"

ditto -x -k "$work/$zip" "$work/extracted"
[ -d "$work/extracted/$APP" ] || die "$zip does not contain $APP"

if [ -d "$install_dir" ] && [ -w "$install_dir" ]; then
  run=""
else
  run="sudo"
fi

stage="$install_dir/.Flock-install-$$.app"
old="$install_dir/.Flock-old-$$.app"
$run mkdir -p "$install_dir"
$run rm -rf "$stage" "$old"
$run ditto "$work/extracted/$APP" "$stage"
if [ -e "$install_dir/$APP" ]; then
  $run mv "$install_dir/$APP" "$old"
fi
$run mv "$stage" "$install_dir/$APP"
$run rm -rf "$old"

$run xattr -dr com.apple.quarantine "$install_dir/$APP" 2>/dev/null || true
codesign --verify --deep --strict "$install_dir/$APP" || die "code signature check failed"

printf 'Installed Flock %s to %s/%s\n' "$version" "$install_dir" "$APP"
