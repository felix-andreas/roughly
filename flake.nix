{
  inputs = {
    systems.url = "github:nix-systems/default";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
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
              ./types
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

          # The dep-only builds compile dependencies against a dummified copy
          # of the workspace: every local .rs file is replaced by an empty
          # stub so the dependency cache survives source edits. The
          # [patch.crates-io] crate in patches/ must keep its REAL sources in
          # that dummy copy — chrono compiles against it — so it is restored
          # verbatim. Interpolating only ./patches keeps the dummy source
          # independent of the rest of the tree, preserving the cache.
          keepPatchesInDummySrc = ''
            rm -rf "$out"/patches
            cp -r --no-preserve=mode ${./patches} "$out"/patches
          '';

          cargoArtifactsLinux = craneLib.buildDepsOnly (
            commonArgsLinux
            // {
              extraDummyScript = keepPatchesInDummySrc;
            }
          );
          packageLinux = craneLib.buildPackage (
            commonArgsLinux
            // {
              cargoArtifacts = cargoArtifactsLinux;
            }
          );

          # The macOS binaries cross-link with zig, which bundles link stubs
          # for libSystem only — no Apple frameworks. The dependency graph is
          # kept framework-free on purpose (see patches/iana-time-zone and the
          # preflight in the justfile's release recipe), so no macOS SDK is
          # needed here.
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
            };

          makeCrossArtifacts =
            target:
            craneLib.buildDepsOnly (
              (makeCrossArgs target)
              // {
                extraDummyScript = keepPatchesInDummySrc;
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
