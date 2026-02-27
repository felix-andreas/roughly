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
            # pak
            devtools
            # rextendr
            # usethis
          ];
        rustToolchain =
          pkgs:
          pkgs.rust-bin.selectLatestNightlyWith (
            toolchain:
            toolchain.default.override {
              targets = [
                "x86_64-unknown-linux-gnu"
                "x86_64-pc-windows-gnu"
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
          src = craneLib.cleanCargoSource ./.;

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

            # C compiler for the `cc` crate (tree-sitter) and linker for
            # the target triple.
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
        }
      );

      devShells = self.lib.eachSystem (system: {
        default =
          let
            pkgs = self.lib.makePkgs system nixpkgs;
            pkgsWin64 = pkgs.pkgsCross.mingwW64;
          in
          # pkgs.devshell.mkShell {
          # required for depsBuildBuild
          pkgs.mkShell {
            motd = "";
            buildInputs = [ pkgs.bashInteractive ];
            depsBuildBuild = [
              pkgsWin64.stdenv.cc
              # pkgsWin64.windows.mingw_w64_pthreads # disabled because of nix error
            ];
            # TODO: we need to link R for rofy
            # nativeBuildInputs = [
            #   (pkgsWin64.rWrapper.override {
            #     packages = [ pkgsWin64.R ];
            #     threads = pkgsWin64.windows.pthreads;
            #   })
            # ];
            # TODO: fixes issue undefined reference to `ts_node_end_byte' in tree-sitter
            # maybe we want a separate derivation to build for windows??
            TARGET_CC = "${pkgsWin64.stdenv.cc}/bin/${pkgsWin64.stdenv.cc.targetPrefix}cc";
            # from here https://github.com/NixOS/nixpkgs/pull/457066/changes
            RUSTFLAGS = "-L native=${pkgsWin64.windows.pthreads}/lib";
            packages = with pkgs; [
              just
              (radianWrapper.override {
                packages = (self.lib.rpkgs pkgs);
                wrapR = true;
              })
              gnumake
              evcxr
              (self.lib.rustToolchain pkgs)
              cargo-edit
              cargo-insta
              bun
              tree-sitter
              # for releasing
              zip
              # libs
              # pkg-config
              # used for std
              # openssl
              # zlib
            ];
            # env = [
            #   {
            #     name = "LD_LIBRARY_PATH";
            #     prefix = "$DEVSHELL_DIR/lib";
            #   }
            #   {
            #     name = "PKG_CONFIG_PATH";
            #     prefix = "$DEVSHELL_DIR/lib/pkgconfig";
            #   }
            # ];
          };
      });
    };
}
