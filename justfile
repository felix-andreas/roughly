#
# DEVELOPMENT
#

default:
  @just --list

roughly *args:
	@cargo run -q -- {{args}}

rofy *args:
	@cargo run -p rofy -q -- {{args}}

test *args:
	cargo test -- --nocapture {{args}}

test-docs:
	cargo test --test test_format -- --no-capture docs

snapshot *args:
	cargo insta test --review -- --nocapture {{args}}

snapshot-delete-unreferenced:
	cargo insta test --unreferenced delete

docs:
	cd docs && bun dev

vsce *args:
	@bun --cwd=editors/code run vsce -- {{args}}

install-extension:
	mkdir -p release/dev
	just build-extension pre-release --out ../../release/dev/roughly.vsix
	code --install-extension release/dev/roughly.vsix

#
# BUILD
#

build:
	cargo build

build-linux:
	cargo build --release --target x86_64-unknown-linux-gnu

build-windows:
	cargo build --release --target x86_64-pc-windows-gnu

build-extension $kind *args:
	#!/usr/bin/env bash
	set -euo pipefail

	release_flag=$(just vsce-release-flag $kind)
	cp LICENSE editors/code
	just vsce package $release_flag {{args}}

#
# RELEASE
#

bump-version $version:
	#!/usr/bin/env bash
	set -euo pipefail

	sed -i 's/^version = "[a-zA-Z0-9._-]*"/version = "{{version}}"/' Cargo.toml
	sed -i 's/"version": "[a-zA-Z0-9._-]*"/"version": "{{version}}"/' editors/code/package.json
	sed -i 's/^version = "[a-zA-Z0-9._-]*"/version = "{{version}}"/' editors/zed/extension.toml
	cargo check # bonus: also updates version in lock file
	git add Cargo.toml Cargo.lock editors/code/package.json editors/zed/extension.toml
	git commit -m "chore: Release v{{version}}"

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

	if [ -z "{{version}}" ]; then
		version=$(git rev-parse --short=6 HEAD)
		echo "info: using git revision $version as version"
	fi
	just release $version pre-release
	just publish-github $version

release $version $kind:
	#!/usr/bin/env bash
	set -euo pipefail

	dir=release/$version

	mkdir -p release
	rm -rf $dir
	mkdir -p $dir
	
	# check (e.g. to test if zed extension works)
	cargo check

	# build server (x86_64-unknown-linux-gnu)
	just build-linux
	mkdir -p $dir/roughly-x86_64-unknown-linux-gnu
	cp target/x86_64-unknown-linux-gnu/release/roughly $dir/roughly-x86_64-unknown-linux-gnu/roughly
	tar -czf $dir/roughly-x86_64-unknown-linux-gnu.tar.gz -C $dir/roughly-x86_64-unknown-linux-gnu roughly

	# build server (x86_64-pc-windows-gnu)
	just build-windows
	mkdir -p $dir/roughly-x86_64-pc-windows-gnu
	cp target/x86_64-pc-windows-gnu/release/roughly.exe $dir/roughly-x86_64-pc-windows-gnu/roughly.exe
	zip -j $dir/roughly-x86_64-pc-windows-gnu.zip $dir/roughly-x86_64-pc-windows-gnu/roughly.exe

	# build vscode extension (client only)
	rm -rf editors/code/bin
	just build-extension $kind --out ../../$dir/roughly.vsix

	# build vscode extension (linux-x64)
	rm -rf editors/code/bin
	mkdir -p editors/code/bin
	cp $dir/roughly-x86_64-unknown-linux-gnu/roughly editors/code/bin
	just build-extension $kind --target linux-x64 --out ../../$dir/roughly-linux-x64.vsix

	# build vscode extension (win32-x64)
	rm -rf editors/code/bin
	mkdir -p editors/code/bin
	cp $dir/roughly-x86_64-pc-windows-gnu/roughly.exe editors/code/bin
	just build-extension $kind --target win32-x64 --out ../../$dir/roughly-win32-x64.vsix

publish-github $version:
	#!/usr/bin/env bash
	set -euo pipefail

	git push
	gh release create $version \
		"release/$version/roughly-x86_64-unknown-linux-gnu.tar.gz#Roughly CLI (linux-x64)" \
		"release/$version/roughly-x86_64-pc-windows-gnu.zip#Roughly CLI (win32-x64)" \
		"release/$version/roughly.vsix#VS Code extension (client only)" \
		"release/$version/roughly-linux-x64.vsix#VS Code extension (linux-x64)" \
		"release/$version/roughly-win32-x64.vsix#VS Code extension (win32-x64)" \
		--notes ""

publish-github-update $version:
	gh release upload $version \
		"release/$version/roughly-x86_64-unknown-linux-gnu.tar.gz#Roughly CLI (linux-x64)" \
		"release/$version/roughly-x86_64-pc-windows-gnu.zip#Roughly CLI (win32-x64)" \
		"release/$version/roughly.vsix#VS Code extension (client only)" \
		"release/$version/roughly-linux-x64.vsix#VS Code extension (linux-x64)" \
		"release/$version/roughly-win32-x64.vsix#VS Code extension (win32-x64)" \
		--clobber

publish-marketplace $version $kind:
	#!/usr/bin/env bash
	set -euo pipefail

	release_flag=$(just vsce-release-flag $kind)

	just vsce publish $release_flag --packagePath ../../release/$version/roughly-linux-x64.vsix
	just vsce publish $release_flag --packagePath ../../release/$version/roughly-win32-x64.vsix
	just vsce publish $release_flag --packagePath ../../release/$version/roughly.vsix

#
# UTILS
#

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
	cd .local/rlib && for dir in */; do (echo "$dir" && cd "$dir" && git {{args}}); done

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
