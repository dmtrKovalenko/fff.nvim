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

          nativeBuildInputs = [ pkgs.pkg-config pkgs.perl ];
          buildInputs = with pkgs; [
            # Add additional build inputs here
            openssl
          ];
        };

        my-crate = craneLib.buildPackage (
          commonArgs
          // {
            cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          }
        );
        # Create a script that copies the build result
        release-script = pkgs.writeShellScriptBin "release" ''
          set -euo pipefail
          nix build
          mkdir -p target/release
          if [ "$(uname)" = "Darwin" ]; then
            cp -vf result/lib/libfff_nvim.dylib target/release/libfff_nvim.dylib
          else
            cp -vf result/lib/libfff_nvim.so    target/release/libfff_nvim.so
          fi
          rm result
          echo "Library copied to target/release/"
        '';
      in
      {
        checks = {
          inherit my-crate;
        };

        packages.default = my-crate;

        apps.default = flake-utils.lib.mkApp {
          drv = my-crate;
        };

        # Add the release command
        apps.release = flake-utils.lib.mkApp {
          drv = release-script;
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
