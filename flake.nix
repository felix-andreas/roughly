{
  inputs = {
    systems.url = "github:nix-systems/default";
    nixpkgs.url = "nixpkgs"; # local registry
    devshell = {
      url = "github:numtide/devshell";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      systems,
      nixpkgs,
      devshell,
      rust-overlay,
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
      };
      devShells = self.lib.eachSystem (system: {
        default =
          let
            pkgs = self.lib.makePkgs system nixpkgs;
            pkgsWin = pkgs.pkgsCross.mingwW64;
            pkgsMac = pkgs.pkgsCross.aarch64-darwin;
          in
          # pkgs.devshell.mkShell {
          # required for depsBuildBuild
          pkgs.mkShell {
            motd = "";
            buildInputs = [ pkgs.bashInteractive ];
            depsBuildBuild = [
              # pkgsMac.stdenv.libiconv
              # pkgsWin.stdenv.cc
              # pkgsWin.windows.pthreads
              # pkgsMac.stdenv.cc
            ];
            # TODO: we need to link R for rofy
            # nativeBuildInputs = [
            #   (pkgsWin.rWrapper.override {
            #     packages = [ pkgsWin.R ];
            #     threads = pkgsWin.windows.pthreads;
            #   })
            # ];
            # TODO: fixes issue undefined reference to `ts_node_end_byte' in tree-sitter
            # maybe we want a separate derivation to build for windows??
            # TARGET_CC = "${pkgsWin.stdenv.cc}/bin/${pkgsWin.stdenv.cc.targetPrefix}cc";
            TARGET_CC = "${pkgsMac.stdenv.cc}/bin/${pkgsMac.stdenv.cc.targetPrefix}cc";
            packages = with pkgs; [
              pkgsMac.clang
              just
              (radianWrapper.override {
                packages = (self.lib.rpkgs pkgs);
                wrapR = true;
              })
              gnumake
              evcxr
              (pkgs.rust-bin.selectLatestNightlyWith (
                toolchain:
                toolchain.default.override {
                  targets = [
                    # "x86_64-unknown-linux-gnu"
                    # "x86_64-pc-windows-gnu"
                    "aarch64-apple-darwin"
                  ];
                  extensions = [
                    "rust-src"
                    "rust-analyzer"
                  ];
                }
              ))
              cargo-edit
              cargo-insta
              bun
              tree-sitter
              # libs
              # pkg-config
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
