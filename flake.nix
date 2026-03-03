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
          pkgsWin64 = pkgs.pkgsCross.mingwW64;
          rustToolchain = self.lib.rustToolchain pkgs;

          craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
          unfilteredRoot = ./.;

          src = pkgs.lib.fileset.toSource {
            root = unfilteredRoot;
            fileset = pkgs.lib.fileset.unions [
              (craneLib.fileset.commonCargoSources unfilteredRoot)
              ./crates/roughly/tests/format
              ./crates/roughly/tests/snapshots
            ];
          };

          commonArgs = {
            inherit src;
            pname = "roughly";
            strictDeps = true;
            cargoExtraArgs = "-p roughly";
          };

          commonArgsLinux = commonArgs // {
            CARGO_BUILD_TARGET = "x86_64-unknown-linux-gnu";
          };

          cargoArtifactsLinux = craneLib.buildDepsOnly commonArgsLinux;

          commonArgsWindows = commonArgs // {
            CARGO_BUILD_TARGET = "x86_64-pc-windows-gnu";

            depsBuildBuild = [
              pkgsWin64.stdenv.cc
            ];

            # C compiler for the `cc` crate (tree-sitter) and linker for the target triple.
            TARGET_CC = "${pkgsWin64.stdenv.cc}/bin/${pkgsWin64.stdenv.cc.targetPrefix}cc";
            CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "${pkgsWin64.stdenv.cc}/bin/${pkgsWin64.stdenv.cc.targetPrefix}cc";

            # Make sure pthreads can be found when linking.
            CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-L native=${pkgsWin64.windows.pthreads}/lib";

            # Host C compiler (needed by build scripts that run on the
            # build machine).
            HOST_CC = "${pkgs.stdenv.cc}/bin/cc";

            # We can't execute Windows binaries on Linux.
            doCheck = false;
          };

          cargoArtifactsWindows = craneLib.buildDepsOnly commonArgsWindows;

          commonArgsMacOS = commonArgs // {
            CARGO_BUILD_TARGET = "aarch64-apple-darwin";

            nativeBuildInputs = [
              pkgs.cargo-zigbuild
              pkgs.zig
            ];

            preBuild = ''
              export XDG_CACHE_HOME="$TMPDIR/.cache"
            '';

            # We can't execute macOS binaries on Linux.
            doCheck = false;
          };

          cargoArtifactsMacOS = craneLib.buildDepsOnly (
            commonArgsMacOS
            // {
              buildPhaseCargoCommand = "cargo zigbuild --release -p roughly";
              checkPhaseCargoCommand = "true";
            }
          );
        in
        {
          default = self.packages.${system}.roughly-linux-x86_64;

          roughly-linux-x86_64 = craneLib.buildPackage (
            commonArgsLinux
            // {
              cargoArtifacts = cargoArtifactsLinux;
            }
          );

          roughly-windows-x86_64 = craneLib.buildPackage (
            commonArgsWindows
            // {
              cargoArtifacts = cargoArtifactsWindows;
            }
          );

          roughly-macos-aarch64 = craneLib.buildPackage (
            commonArgsMacOS
            // {
              cargoArtifacts = cargoArtifactsMacOS;
              buildPhaseCargoCommand = ''
                cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
                cargo zigbuild --release --message-format json-render-diagnostics -p roughly >"$cargoBuildLog"
              '';
            }
          );
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
