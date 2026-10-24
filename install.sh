#!/bin/sh
set -eu

main() {
    case "$(uname -sm)" in
        'Linux x86_64') asset=likh-linux-x86_64 ;;
        *) echo 'Likh releases support Linux x86_64 only.' >&2; exit 1 ;;
    esac

    dir=${LIKH_INSTALL_DIR:-"$HOME/.local/bin"}
    url=https://github.com/pkkulhari/likh/releases/latest/download
    mkdir -p -- "$dir"
    tmp=$(mktemp -d "$dir/.likh-install.XXXXXX")
    trap 'rm -rf -- "$tmp"' 0
    trap 'exit 1' HUP INT TERM

    curl -fsSL "$url/$asset" -o "$tmp/$asset"
    curl -fsSL "$url/$asset.sha256" -o "$tmp/$asset.sha256"
    (cd "$tmp" && sha256sum -c "$asset.sha256")
    chmod 755 "$tmp/$asset"
    version=$("$tmp/$asset" --version)
    mv -fT -- "$tmp/$asset" "$dir/likh"

    printf 'Installed %s to %s/likh\n' "$version" "$dir"
    case ":$PATH:" in
        *":$dir:"*) ;;
        *) printf 'Add %s to your PATH to use the likh command.\n' "$dir" ;;
    esac
    echo 'First install: run likh doctor, likh login, then likh run.'
    echo 'After an update, restart any running Likh instance.'
}

main
