#!/bin/sh
# Installs nexo (macOS, Linux) from the GitHub releases, no Rust needed:
#
#   curl -fsSL https://raw.githubusercontent.com/Qwizi/nexo-cli/main/install.sh | sh
#
# Settings (environment):
#   NEXO_VERSION      a tag such as v0.2.0 (default: the latest release)
#   NEXO_INSTALL_DIR  where the binary goes (default: ~/.local/bin)
#   NEXO_URL          panel address to sign in to right after installing
#   NEXO_NO_LOGIN=1   skip the sign-in question
#   GH_TOKEN          only while the repository is private (or a signed-in `gh`)
set -eu

REPO="Qwizi/nexo-cli"
API="https://api.github.com/repos/$REPO"

say() { printf '%s\n' "$*" >&2; }
fail() { say "błąd: $*"; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "brak programu $1"; }

need curl
need tar
need uname

case "$(uname -s)" in
	Darwin) os=apple-darwin ;;
	Linux) os=unknown-linux-musl ;;
	*) fail "nieobsługiwany system $(uname -s); na Windows użyj install.ps1" ;;
esac
case "$(uname -m)" in
	x86_64 | amd64) arch=x86_64 ;;
	arm64 | aarch64) arch=aarch64 ;;
	*) fail "nieobsługiwany procesor $(uname -m)" ;;
esac
# Rosetta: an Intel shell on Apple silicon still gets the native binary.
if [ "$os" = apple-darwin ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
	arch=aarch64
fi
target="$arch-$os"

token="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
if [ -z "$token" ] && command -v gh >/dev/null 2>&1; then
	token="$(gh auth token 2>/dev/null || true)"
fi
# api ACCEPT URL [curl options…]: one Accept header (JSON or the raw file).
api() {
	accept="$1"
	shift
	if [ -n "$token" ]; then
		curl -fsSL -H "Authorization: Bearer $token" -H "Accept: $accept" "$@"
	else
		curl -fsSL -H "Accept: $accept" "$@"
	fi
}
json=application/vnd.github+json

if [ -n "${NEXO_VERSION:-}" ]; then
	release="$(api "$json" "$API/releases/tags/$NEXO_VERSION")" || fail "nie ma wydania $NEXO_VERSION"
else
	release="$(api "$json" "$API/releases/latest")" || fail "nie mogę pobrać informacji o wydaniu (prywatne repo? ustaw GH_TOKEN albo zaloguj gh)"
fi
tag="$(printf '%s' "$release" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
[ -n "$tag" ] || fail "nie znalazłem wydania"
archive="nexo-$tag-$target.tar.gz"

# id of an asset of the release, for the API download (works for private repos)
asset_id() {
	printf '%s' "$release" | tr ',' '\n' | awk -v name="$1" '
		/"id":/ { gsub(/[^0-9]/, "", $0); id = $0 }
		/"name":/ { if (index($0, "\"" name "\"")) { print id; exit } }'
}
download() {
	id="$(asset_id "$1")"
	[ -n "$id" ] || fail "wydanie $tag nie ma pliku $1"
	api application/octet-stream "$API/releases/assets/$id" -o "$2"
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM
say "Pobieram nexo $tag ($target)…"
download "$archive" "$tmp/$archive"
download SHA256SUMS "$tmp/SHA256SUMS"

expected="$(grep " $archive\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)"
if command -v sha256sum >/dev/null 2>&1; then
	actual="$(sha256sum "$tmp/$archive" | cut -d' ' -f1)"
else
	actual="$(shasum -a 256 "$tmp/$archive" | cut -d' ' -f1)"
fi
[ -n "$expected" ] && [ "$expected" = "$actual" ] || fail "suma kontrolna się nie zgadza"

tar -xzf "$tmp/$archive" -C "$tmp"
dir="${NEXO_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$dir"
install -m 755 "$tmp/nexo-$tag-$target/nexo" "$dir/nexo" 2>/dev/null || {
	cp "$tmp/nexo-$tag-$target/nexo" "$dir/nexo"
	chmod 755 "$dir/nexo"
}
say "Zainstalowano $dir/nexo"

case ":$PATH:" in
	*":$dir:"*) ;;
	*)
		line="export PATH=\"$dir:\$PATH\""
		case "${SHELL:-}" in
			*zsh) rc="$HOME/.zshrc" ;;
			*bash) if [ "$os" = apple-darwin ]; then rc="$HOME/.bash_profile"; else rc="$HOME/.bashrc"; fi ;;
			*) rc="$HOME/.profile" ;;
		esac
		if ! grep -qsF "$line" "$rc"; then
			printf '\n# nexo\n%s\n' "$line" >>"$rc"
			say "Dodałem $dir do PATH w $rc (nowy terminal albo: . $rc)"
		fi
		PATH="$dir:$PATH"
		;;
esac

[ "${NEXO_NO_LOGIN:-}" = 1 ] && exit 0
url="${NEXO_URL:-}"
if [ -z "$url" ] && [ -r /dev/tty ] && [ -w /dev/tty ]; then
	printf 'Adres panelu CMS do zalogowania (np. https://cms.twoja-domena.pl, Enter = później): ' >/dev/tty
	read -r url </dev/tty || url=""
fi
if [ -n "$url" ]; then
	"$dir/nexo" login --url "$url" </dev/tty
else
	say "Zaloguj się później: nexo login"
fi
