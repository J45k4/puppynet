#!/usr/bin/env sh
set -eu

repo="J45k4/puppynet"
install_dir="${PUPPYNET_INSTALL_DIR:-}"
version="${PUPPYNET_VERSION:-latest}"
install_service=1
install_mode="user"

usage() {
	printf '%s\n' "Usage: install.sh [--system] [--no-service] [--version VERSION] [--install-dir DIR]"
}

need_cmd() {
	if ! command -v "$1" >/dev/null 2>&1; then
		printf '%s\n' "error: required command not found: $1" >&2
		exit 1
	fi
}

parse_args() {
	while [ "$#" -gt 0 ]; do
		case "$1" in
			--system)
				install_mode="system"
				;;
			--no-service)
				install_service=0
				;;
			--version)
				shift
				if [ "$#" -eq 0 ]; then
					printf '%s\n' "error: --version requires a value" >&2
					exit 1
				fi
				version="$1"
				;;
			--install-dir)
				shift
				if [ "$#" -eq 0 ]; then
					printf '%s\n' "error: --install-dir requires a value" >&2
					exit 1
				fi
				install_dir="$1"
				;;
			-h|--help)
				usage
				exit 0
				;;
			*)
				printf '%s\n' "error: unknown argument: $1" >&2
				usage >&2
				exit 1
				;;
		esac
		shift
	done
}

set_default_install_dir() {
	if [ -n "$install_dir" ]; then
		return
	fi

	if [ "$install_mode" = "system" ]; then
		install_dir="/usr/local/bin"
	else
		install_dir="$HOME/.puppynet/bin"
	fi
}

detect_platform() {
	case "$(uname -s)" in
		Linux)
			printf '%s\n' "linux"
			;;
		Darwin)
			printf '%s\n' "macos"
			;;
		*)
			printf '%s\n' "error: unsupported operating system: $(uname -s)" >&2
			exit 1
			;;
	esac
}

as_root() {
	if [ "$(id -u)" -eq 0 ]; then
		"$@"
		return
	fi

	if command -v sudo >/dev/null 2>&1; then
		sudo "$@"
		return
	fi

	printf '%s\n' "error: --system requires root or sudo" >&2
	exit 1
}

latest_version() {
	curl -fsSL "https://api.github.com/repos/$repo/releases/latest" \
		| sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
		| head -n 1
}

download_asset() {
	asset="$1"
	url="https://github.com/$repo/releases/download/$version/$asset"
	printf '%s\n' "Downloading $url"
	curl -fL "$url" -o "$archive"
}

install_binary() {
	tar -xzf "$archive" -C "$tmpdir" puppynet
	chmod 755 "$tmpdir/puppynet"

	if [ "$install_mode" = "system" ]; then
		as_root mkdir -p "$install_dir"
		as_root rm -f "$install_dir/puppynet.new"
		as_root cp "$tmpdir/puppynet" "$install_dir/puppynet.new"
		as_root chmod 755 "$install_dir/puppynet.new"
		as_root mv "$install_dir/puppynet.new" "$install_dir/puppynet"
	else
		mkdir -p "$install_dir"
		if [ -f "$install_dir/puppynet.new" ]; then
			rm -f "$install_dir/puppynet.new"
		fi
		cp "$tmpdir/puppynet" "$install_dir/puppynet.new"
		mv "$install_dir/puppynet.new" "$install_dir/puppynet"
	fi

	printf '%s\n' "Installed puppynet to $install_dir/puppynet"
}

link_user_binary() {
	if [ "$install_mode" != "user" ]; then
		return
	fi

	mkdir -p "$HOME/.local/bin"
	ln -sf "$install_dir/puppynet" "$HOME/.local/bin/puppynet"
	printf '%s\n' "Linked puppynet to $HOME/.local/bin/puppynet"

	case ":$PATH:" in
		*":$HOME/.local/bin:"*)
			;;
		*)
			printf '%s\n' "warning: $HOME/.local/bin is not on PATH" >&2
			printf '%s\n' 'Add this to your shell profile: export PATH="$HOME/.local/bin:$PATH"' >&2
			;;
	esac
}

install_puppynet_service() {
	if [ "$install_service" -eq 0 ]; then
		return
	fi

	if [ "$install_mode" = "system" ]; then
		printf '%s\n' "Installing puppynet system service"
		if as_root "$install_dir/puppynet" install --system; then
			printf '%s\n' "PuppyNet system service installed and started"
			return
		fi

		printf '%s\n' "warning: system service install failed; binary install completed" >&2
		printf '%s\n' "Run manually if needed: sudo $install_dir/puppynet install --system" >&2
		return
	fi

	printf '%s\n' "Installing puppynet user service"
	if "$install_dir/puppynet" install; then
		printf '%s\n' "PuppyNet service installed and started"
		return
	fi

	printf '%s\n' "warning: service install failed; binary install completed" >&2
	printf '%s\n' "Run manually if needed: $install_dir/puppynet install" >&2
}

cleanup() {
	if [ -n "${tmpdir:-}" ] && [ -d "$tmpdir" ]; then
		rm -rf "$tmpdir"
	fi
}

main() {
	parse_args "$@"
	set_default_install_dir
	need_cmd curl
	need_cmd id
	need_cmd mktemp
	need_cmd sed
	need_cmd tar
	need_cmd uname

	platform="$(detect_platform)"
	if [ "$version" = "latest" ]; then
		version="$(latest_version)"
		if [ -z "$version" ]; then
			printf '%s\n' "error: failed to resolve latest PuppyNet release" >&2
			exit 1
		fi
	fi

	tmpdir="$(mktemp -d)"
	trap cleanup EXIT INT TERM
	archive="$tmpdir/puppynet.tar.gz"

	download_asset "puppynet-$version-$platform.tar.gz"
	install_binary
	link_user_binary
	install_puppynet_service

	printf '%s\n' "Done."
}

main "$@"
