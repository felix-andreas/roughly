run *args:
	@cargo run -q -- {{args}}

fmt *args:
	@cargo run -q -- fmt {{args}}

lint *args:
	@cargo run -q -- lint {{args}}

test *args:
	cargo test -- --nocapture {{args}}

snapshot *args:
	cargo insta test --review -- --nocapture {{args}}

bundle *args:
	cd client && npm run package -- {{args}}

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
	sed -i 's/"version": "[ta-zA-Z0-9._-]*"/"version": "{{version}}"/' package.json
	sed -i 's/"version": "[a-zA-Z0-9._-]*"/"version": "{{version}}"/' client/package.json
	sed -i 's/^version = "[a-zA-Z0-9._-]*"/version = "{{version}}"/' Cargo.toml
	cargo check # bonus: also updates version in lock file
	git add package.json client/package.json Cargo.toml Cargo.lock
	git commit -m "chore: Release v{{version}}"

build-release $version="":
	#!/usr/bin/env bash
	set -euo pipefail
	if [ -z "{{version}}" ]; then
		version=$(git rev-parse --short=6 HEAD)
		echo "info: using git revision $version as version"
	fi
	rm -rf release
	mkdir release
	# build client
	just bundle --out ../release/roughly-$version.vsix
	# build server
	just linux
	just windows
	cp target/release/roughly release/roughly-$version
	cp target/x86_64-pc-windows-gnu/release/roughly.exe release/roughly-$version.exe

publish-release $version:
	#!/usr/bin/env bash
	set -euo pipefail
	git push
	gh release create $version \
		"release/roughly-$version.vsix#VS Code extension" \
		"release/roughly-$version#LSP server (Linux)" \
		"release/roughly-$version.exe#LSP server (Windows)" \
		--notes "" \
		--prerelease

update-release $version:
	gh release upload $version \
		"release/roughly-$version.vsix#VS Code extension" \
		"release/roughly-$version#LSP server (Linux)" \
		"release/roughly-$version.exe#LSP server (Windows)" \
		--clobber
