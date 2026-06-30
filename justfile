#
# DEVELOPMENT
#

default:
    @just --list

roughly *args:
    @cargo run -q -- {{ args }}

rofy *args:
    @cargo run -p rofy -q -- {{ args }}

test *args:
    cargo test -- --nocapture {{ args }}

test-docs:
    cargo test --test test_format -- --no-capture docs

test-analysis filter="" *args:
    FIXTURE_FILTER={{ filter }} cargo nextest run -p analysis --test test_fixtures {{ args }}

bench *args:
    cargo test --release -p analysis --test test_benchmark -- --ignored --nocapture {{ args }}

docs:
    cd docs && bun dev

vsce *args:
    @bun --cwd=editors/code run vsce -- {{ args }}

install-extension:
    mkdir -p release/dev
    just build-extension pre-release --out ../../release/dev/roughly.vsix
    code --install-extension release/dev/roughly.vsix

#
# BUILD
#

build:
    cargo build

zigbuild $target *args:
    cargo zigbuild --target {{ target }} {{ args }}

build-platform-vsix target vscode-target binary-name dir kind:
    rm -rf editors/code/bin
    mkdir -p editors/code/bin
    cp {{ dir }}/roughly-{{ target }}/{{ binary-name }} editors/code/bin
    just build-extension {{ kind }} --target {{ vscode-target }} --out ../../{{ dir }}/roughly-{{ vscode-target }}.vsix

build-extension $kind *args:
    #!/usr/bin/env bash
    set -euo pipefail

    release_flag=$(just vsce-release-flag $kind)
    cp LICENSE editors/code
    just vsce package $release_flag {{ args }}

#
# RELEASE
#

publish $version $kind:
    #!/usr/bin/env bash
    set -euo pipefail

    just bump-version $version
    just release $version $kind
    just publish-github $version
    just publish-marketplace $version $kind

publish-commit $version="":
    #!/usr/bin/env bash
    set -euo pipefail

    if [ -z "{{ version }}" ]; then
    	version=$(git rev-parse --short=6 HEAD)
    	echo "info: using git revision $version as version"
    fi
    just release $version pre-release
    just publish-github $version

bump-version $version:
    #!/usr/bin/env bash
    set -euo pipefail

    sed -i 's/^version = "[a-zA-Z0-9._-]*"/version = "{{ version }}"/' Cargo.toml

    # Versioning schema, and the marketplace constraint that drives it:
    #   The VS Marketplace only accepts a plain `X.Y.Z` and REJECTS semver pre-release
    #   suffixes such as `-alpha.3`. So releases use this schema:
    #     pre-release : `X.Y.Z-alpha` / `X.Y.Z-beta`  — the PATCH (Z) is a single
    #                   monotonically increasing counter and the suffix names the
    #                   channel, e.g. 0.2.1-alpha, 0.2.2-alpha, ..., 0.2.14-beta.
    #     stable      : a bare `X.Y.Z`.
    #   The extension version is the CLI version with the channel suffix stripped — a
    #   valid, monotonic `X.Y.Z` — and the `--pre-release` flag marks the channel on
    #   the marketplace. Keeping the counter in the PATCH (not the suffix) is what makes
    #   stripping collision-free: 0.2.1-alpha and 0.2.2-alpha map to distinct 0.2.1 / 0.2.2.
    cli_version="{{ version }}"
    vscode_version="${cli_version%%-*}"
    sed -i "s/\"version\": \"[a-zA-Z0-9._-]*\"/\"version\": \"$vscode_version\"/" editors/code/package.json

    # editors/zed/extension.toml is intentionally NOT version-bumped: the Zed extension
    # does not bundle the binary, so its version is decoupled from the release version.

    cargo check # bonus: also updates version in lock file
    git add Cargo.toml Cargo.lock editors/code/package.json
    git commit -m "chore: Release v{{ version }}"

release $version $kind:
    #!/usr/bin/env bash
    set -euo pipefail

    dir=release/$version

    mkdir -p release
    rm -rf $dir release/nix
    mkdir -p $dir release/nix

    cargo check

    nix build .#roughly-linux-x86_64 -o release/nix/x86_64-unknown-linux-gnu
    nix build .#roughly-macos-aarch64 -o release/nix/aarch64-apple-darwin
    nix build .#roughly-windows-x86_64 -o release/nix/x86_64-pc-windows-gnu

    just package-tar x86_64-unknown-linux-gnu $dir
    just package-tar aarch64-apple-darwin $dir
    just package-zip x86_64-pc-windows-gnu $dir

    # vscode extension (client only)
    rm -rf editors/code/bin
    just build-extension $kind --out ../../$dir/roughly.vsix

    just build-platform-vsix x86_64-unknown-linux-gnu linux-x64 roughly $dir $kind
    just build-platform-vsix aarch64-apple-darwin darwin-arm64 roughly $dir $kind
    just build-platform-vsix x86_64-pc-windows-gnu win32-x64 roughly.exe $dir $kind

publish-github $version:
    #!/usr/bin/env bash
    set -euo pipefail

    git push
    gh release create $version \
    	"release/$version/roughly-x86_64-unknown-linux-gnu.tar.gz#Roughly CLI (linux-x64)" \
    	"release/$version/roughly-aarch64-apple-darwin.tar.gz#Roughly CLI (darwin-arm64)" \
    	"release/$version/roughly-x86_64-pc-windows-gnu.zip#Roughly CLI (win32-x64)" \
    	"release/$version/roughly.vsix#VS Code extension (client only)" \
    	"release/$version/roughly-linux-x64.vsix#VS Code extension (linux-x64)" \
    	"release/$version/roughly-darwin-arm64.vsix#VS Code extension (darwin-arm64)" \
    	"release/$version/roughly-win32-x64.vsix#VS Code extension (win32-x64)" \
    	--notes ""

publish-github-update $version:
    gh release upload $version \
        "release/$version/roughly-x86_64-unknown-linux-gnu.tar.gz#Roughly CLI (linux-x64)" \
        "release/$version/roughly-aarch64-apple-darwin.tar.gz#Roughly CLI (darwin-arm64)" \
        "release/$version/roughly-x86_64-pc-windows-gnu.zip#Roughly CLI (win32-x64)" \
        "release/$version/roughly.vsix#VS Code extension (client only)" \
        "release/$version/roughly-linux-x64.vsix#VS Code extension (linux-x64)" \
        "release/$version/roughly-darwin-arm64.vsix#VS Code extension (darwin-arm64)" \
        "release/$version/roughly-win32-x64.vsix#VS Code extension (win32-x64)" \
        --clobber

publish-marketplace $version $kind:
    #!/usr/bin/env bash
    set -euo pipefail

    release_flag=$(just vsce-release-flag $kind)

    just vsce publish $release_flag --packagePath ../../release/$version/roughly-linux-x64.vsix
    just vsce publish $release_flag --packagePath ../../release/$version/roughly-darwin-arm64.vsix
    just vsce publish $release_flag --packagePath ../../release/$version/roughly-win32-x64.vsix
    just vsce publish $release_flag --packagePath ../../release/$version/roughly.vsix

#
# UTILS
#

package-tar target dir:
    mkdir -p {{ dir }}/roughly-{{ target }}
    cp release/nix/{{ target }}/bin/roughly {{ dir }}/roughly-{{ target }}/roughly
    tar -czf {{ dir }}/roughly-{{ target }}.tar.gz -C {{ dir }}/roughly-{{ target }} roughly

package-zip target dir:
    mkdir -p {{ dir }}/roughly-{{ target }}
    cp release/nix/{{ target }}/bin/roughly.exe {{ dir }}/roughly-{{ target }}/roughly.exe
    zip -j {{ dir }}/roughly-{{ target }}.zip {{ dir }}/roughly-{{ target }}/roughly.exe

# use rlib repos to test formatting
rlib-clone:
    #!/usr/bin/env bash
    mkdir -p .local/rlib
    for repo in devtools lintr actions httr testthat usethis styler pkgdown; do
    	if [ ! -d ".local/rlib/$repo" ]; then
    		echo "cloning $repo..."
    		git clone --depth 1 "https://github.com/r-lib/$repo.git" ".local/rlib/$repo"
    	fi
    done

rlib *args:
    cd .local/rlib && for dir in */; do (echo "$dir" && cd "$dir" && git {{ args }}); done

vsce-release-flag $kind:
    @echo {{ if kind == "release" { "" } else if kind == "pre-release" { "--pre-release" } else { error("kind must be either release or pre-release") } }}

#
# ROFY
#

publish-rofy $version="nightly":
    cargo build -p rofy --release --target x86_64-unknown-linux-gnu
    cargo build -p rofy --release --target x86_64-pc-windows-gnu
    gh release create rofy-$version \
        "target/x86_64-unknown-linux-gnu/release/rofy#rofy (linux-x64)" \
        "target/x86_64-pc-windows-gnu/release/rofy.exe#rofy (win32-x64)" \
        --notes "" \
        --prerelease
