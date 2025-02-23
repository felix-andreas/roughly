{
  inputs = {
    systems.url = "github:nix-systems/default";
    nixpkgs.url = "unstable"; # local registry
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
            overlays = [
              devshell.overlays.default
              (import rust-overlay)
            ];
            allowUnsupportedSystem = true;
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
          in
          # pkgs.devshell.mkShell {
          pkgs.mkShell {
            motd = "";
            depsBuildBuild = with pkgs; [
              pkgsCross.mingwW64.stdenv.cc
              pkgsCross.mingwW64.windows.pthreads
            ];
            # TODO: fixes issue undefined reference to `ts_node_end_byte' in tree-sitter
            # maybe we want a separate derivation to build for windows??
            TARGET_CC = "${pkgs.pkgsCross.mingwW64.stdenv.cc}/bin/${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}cc";
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
              cargo-insta
              nodejs
              bun
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
