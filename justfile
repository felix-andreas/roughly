run *args:
	@cargo run -q -- {{args}}

fmt *args:
	@cargo run -q -- fmt {{args}}

lint *args:
	@cargo run -q -- lint {{args}}

test *args:
	cargo test -- --nocapture {{args}}

docs:
	cd docs && bun dev

snapshot *args:
	cargo insta test --review -- --nocapture {{args}}

vsce *args:
	@bun --cwd=client run vsce -- {{args}}

vscode *args:
	cp LICENSE client
	@just vsce package {{args}}

install:
	code --install-extension client/roughly-*.vsix --force

linux:
	cargo build --release

windows:
	cargo build --release --target x86_64-pc-windows-gnu

new-release $version:
	#!/usr/bin/env bash
	set -euo pipefail
	just bump-version $version
	just build-release $version
	just publish-release $version

bump-version $version:
	#!/usr/bin/env bash
	set -euo pipefail
	sed -i 's/"version": "[a-zA-Z0-9._-]*"/"version": "{{version}}"/' client/package.json
	sed -i 's/^version = "[a-zA-Z0-9._-]*"/version = "{{version}}"/' Cargo.toml
	cargo check # bonus: also updates version in lock file
	git add client/package.json Cargo.toml Cargo.lock
	git commit -m "chore: Release v{{version}}"

git-release $version="":
	#!/usr/bin/env bash
	set -euo pipefail
	if [ -z "{{version}}" ]; then
		version=$(git rev-parse --short=6 HEAD)
		echo "info: using git revision $version as version"
	fi
	just build-release $version
	just publish-release $version

build-release $version:
	#!/usr/bin/env bash
	set -euo pipefail
	mkdir -p release
	rm -rf release/$version
	mkdir -p release/$version

	# build server
	just linux
	just windows
	cp target/release/roughly release/$version/roughly
	cp target/x86_64-pc-windows-gnu/release/roughly.exe release/$version/roughly.exe

	# build vscode extension (client only) 
	rm -rf client/bin
	just vscode --out ../release/$version/roughly.vsix

	# build vscode extension (linux-x64)
	rm -rf client/bin
	mkdir -p client/bin
	cp release/$version/roughly client/bin
	just vscode --target linux-x64 --out ../release/$version/roughly-linux-x64.vsix

	# build vscode extension (win32-x64)
	rm -rf client/bin
	mkdir -p client/bin
	cp release/$version/roughly.exe client/bin
	just vscode --target win32-x64 --out ../release/$version/roughly-win32-x64.vsix

publish-release $version:
	#!/usr/bin/env bash
	set -euo pipefail
	git push
	gh release create $version \
		"release/$version/roughly#Roughly CLI (linux-x64)" \
		"release/$version/roughly.exe#Roughly CLI (win32-x64)" \
		"release/$version/roughly.vsix#VS Code extension (client only)" \
		"release/$version/roughly-linux-x64.vsix#VS Code extension (linux-x64)" \
		"release/$version/roughly-win32-x64.vsix#VS Code extension (win32-x64)" \
		--notes "" \
		--prerelease

update-release $version:
	gh release upload $version \
		"release/$version/roughly#Roughly CLI (linux-x64)" \
		"release/$version/roughly.exe#Roughly CLI (win32-x64)" \
		"release/$version/roughly.vsix#VS Code extension (client only)" \
		"release/$version/roughly-linux-x64.vsix#VS Code extension (linux-x64)" \
		"release/$version/roughly-win32-x64.vsix#VS Code extension (win32-x64)" \
		--clobber

publish-marketplace $version:
	@just vsce publish --packagePath ../release/$version/roughly-linux-x64.vsix
	@just vsce publish --packagePath ../release/$version/roughly-win32-x64.vsix
	@just vsce publish --packagePath ../release/$version/roughly.vsix

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
