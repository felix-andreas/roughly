{
  inputs = {
    systems.url = "github:nix-systems/default";
    nixpkgs.url = "nixpkgs"; # local registry
    devshell.url = "github:numtide/devshell";
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
            pkgsWin64 = pkgs.pkgsCross.mingwW64;
          in
          # pkgs.devshell.mkShell {
          # required for depsBuildBuild
          pkgs.mkShell {
            motd = "";
            buildInputs = [ pkgs.bashInteractive ];
            depsBuildBuild = [
              pkgsWin64.stdenv.cc
              pkgsWin64.windows.pthreads
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
            packages = with pkgs; [
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
                  targets = [ "x86_64-pc-windows-gnu" ];
                  extensions = [
                    "rust-src"
                    "rust-analyzer"
                  ];
                }
              ))
              cargo-edit
              cargo-insta
              nodejs
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
