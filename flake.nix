{
  inputs = {
    systems.url = "github:nix-systems/default";
    nixpkgs.url = "nixpkgs"; # local registry
    devshell.url = "github:numtide/devshell";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      systems,
      nixpkgs,
      devshell,
      rust-overlay,
      crane,
    }:
    {
      lib = {
        eachSystem = nixpkgs.lib.genAttrs (import systems);
        makePkgs =
          system: nixpkgs':
          import nixpkgs' {
            inherit system;
            config.allowUnsupportedSystem = true;
            overlays = [
              devshell.overlays.default
              (import rust-overlay)
            ];
          };
        rpkgs =
          pkgs: with pkgs.rPackages; [
            renv
            devtools
          ];
        rustToolchain =
          pkgs:
          pkgs.rust-bin.selectLatestNightlyWith (
            toolchain:
            toolchain.default.override {
              targets = [
                "x86_64-unknown-linux-gnu"
                "x86_64-pc-windows-gnu"
                "aarch64-apple-darwin"
              ];
              extensions = [
                "rust-src"
                "rust-analyzer"
              ];
            }
          );
      };

      packages = self.lib.eachSystem (
        system:
        let
          pkgs = self.lib.makePkgs system nixpkgs;
          rustToolchain = self.lib.rustToolchain pkgs;

          craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
          unfilteredRoot = ./.;

          src = pkgs.lib.fileset.toSource {
            root = unfilteredRoot;
            fileset = pkgs.lib.fileset.unions [
              (craneLib.fileset.commonCargoSources unfilteredRoot)
              # Non-Rust files pulled in by `include_str!` / tests that
              # `commonCargoSources` filters out and must be added explicitly.
              ./crates/semantics/stubs
              ./crates/format/tests/format
            ];
          };

          commonArgs = {
            inherit src;
            pname = "roughly";
            strictDeps = true;
          };

          commonArgsLinux = commonArgs // {
            cargoExtraArgs = "-p roughly";
            CARGO_BUILD_TARGET = "x86_64-unknown-linux-gnu";
          };

          cargoArtifactsLinux = craneLib.buildDepsOnly commonArgsLinux;
          packageLinux = craneLib.buildPackage (
            commonArgsLinux
            // {
              cargoArtifacts = cargoArtifactsLinux;
            }
          );

          # The REPL's line editor transitively links the CoreFoundation
          # framework on macOS (chrono resolves the system time zone through
          # it). zig bundles link stubs for libSystem only, so the cross
          # linker needs a real macOS SDK for framework resolution:
          # cargo-zigbuild picks it up from SDKROOT and passes the framework
          # search paths to zig.
          macosSdk =
            let
              tarball = pkgs.fetchurl {
                url = "https://github.com/joseluisq/macosx-sdks/releases/download/11.3/MacOSX11.3.sdk.tar.xz";
                hash = "sha256-mtwTc9OHnhlz0orZ8XyQUbApMWdKPsKiSYEomJ7OLLE=";
              };
            in
            pkgs.runCommand "MacOSX11.3.sdk" { } ''
              mkdir "$out"
              tar -xf ${tarball} --strip-components=1 -C "$out"
            '';

          makeCrossArgs =
            target:
            commonArgs
            // {
              CARGO_BUILD_TARGET = target;

              nativeBuildInputs = [
                pkgs.cargo-zigbuild
                pkgs.zig
              ];

              preBuild = ''
                export XDG_CACHE_HOME="$TMPDIR/.cache"
              '';

              doCheck = false;
            }
            // pkgs.lib.optionalAttrs (pkgs.lib.hasSuffix "-apple-darwin" target) {
              SDKROOT = macosSdk;
            };

          makeCrossArtifacts =
            target:
            craneLib.buildDepsOnly (
              (makeCrossArgs target)
              // {
                buildPhaseCargoCommand = "cargo zigbuild --release -p roughly";
                checkPhaseCargoCommand = "true";
              }
            );

          buildCrossPackage =
            target:
            craneLib.buildPackage (
              (makeCrossArgs target)
              // {
                cargoArtifacts = makeCrossArtifacts target;
                buildPhaseCargoCommand = ''
                  cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
                  cargo zigbuild --release --message-format json-render-diagnostics -p roughly >"$cargoBuildLog"
                '';
              }
            );
        in
        {
          roughly-linux-x86_64 = packageLinux;
          roughly-macos-aarch64 = buildCrossPackage "aarch64-apple-darwin";
          roughly-windows-x86_64 = buildCrossPackage "x86_64-pc-windows-gnu";
        }
      );

      devShells = self.lib.eachSystem (system: {
        default =
          let
            pkgs = self.lib.makePkgs system nixpkgs;
          in
          pkgs.devshell.mkShell {
            motd = "";
            packages = with pkgs; [
              # development environment
              just
              evcxr
              (radianWrapper.override {
                packages = (self.lib.rpkgs pkgs);
                wrapR = true;
              })
              # build tools
              (self.lib.rustToolchain pkgs)
              gnumake
              cargo-edit
              cargo-insta
              cargo-nextest
              # cross compilation
              cargo-zigbuild
              zig
              # libs
              tree-sitter
              # website
              bun
              # for releasing
              zip
            ];
          };
      });
    };
}
