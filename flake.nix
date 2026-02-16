{
  description = "fff.nvim";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    crane.url = "github:ipetkov/crane";

    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      flake-utils,
      rust-overlay,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Common arguments can be set here to avoid repeating them later
        # Note: changes here will rebuild all dependency crates
        commonArgs = {
          src = craneLib.cleanCargoSource ./.;
          strictDeps = true;

          nativeBuildInputs = [ pkgs.pkg-config pkgs.perl pkgs.zig pkgs.llvmPackages.libclang.lib ];
          buildInputs = with pkgs; [
            # Add additional build inputs here
            openssl
          ];
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        };

        my-crate = craneLib.buildPackage (
          commonArgs
          // {
            cargoArtifacts = craneLib.buildDepsOnly commonArgs;
            doCheck = false;
          }
        );
        # Copies the dynamic library into the target/release folder
        copy-dynamic-library = /* bash */ ''
          set -eo pipefail
          mkdir -p target/release
          if [ "$(uname)" = "Darwin" ]; then
            cp -vf ${my-crate}/lib/libfff_nvim.dylib target/release/libfff_nvim.dylib
          else
            cp -vf ${my-crate}/lib/libfff_nvim.so target/release/libfff_nvim.so
          fi
          echo "Library copied to target/release/"
        '';
      in
      {
        checks = {
          inherit my-crate;
        };

        packages = {
          default = my-crate;

          # Neovim plugin
          fff-nvim = pkgs.vimUtils.buildVimPlugin {
            pname = "fff.nvim";
            version = "main";
            src = pkgs.lib.cleanSource ./.;
            postPatch = copy-dynamic-library;
            doCheck = false; # Skip require check since we have a Rust FFI component
          };
        };

        apps.default = flake-utils.lib.mkApp {
          drv = my-crate;
        };

        # Add the release command
        apps.release = flake-utils.lib.mkApp {
          drv = pkgs.writeShellScriptBin "release" copy-dynamic-library;
        };

        devShells.default = craneLib.devShell {
          # Inherit inputs from checks.
          checks = self.checks.${system};
          # Extra inputs can be added here; cargo and rustc are provided by default.
          packages = [
            # pkgs.ripgrep
          ];
        };
      }
    );
}
