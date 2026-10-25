{
  description = "cargo-frequent: find the cause of frequent cargo rebuilds";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      crane,
      rust-overlay,
      ...
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems =
        buildSystemOutputs:
        nixpkgs.lib.genAttrs supportedSystems (
          system:
          let
            pkgs = import nixpkgs {
              inherit system;
              overlays = [ (import rust-overlay) ];
            };
            toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          in
          buildSystemOutputs {
            inherit pkgs toolchain;
            craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          }
        );
    in
    {
      packages = forAllSystems (
        { craneLib, ... }:
        {
          default = craneLib.buildPackage {
            src = nixpkgs.lib.cleanSourceWith {
              src = ./.;
              name = "source";
              filter =
                sourcePath: sourceType:
                craneLib.filterCargoSources sourcePath sourceType
                || nixpkgs.lib.hasInfix "/e2e-test-crates/" sourcePath;
            };
            strictDeps = true;
            meta.mainProgram = "cargo-frequent";
          };
        }
      );

      devShells = forAllSystems (
        { pkgs, toolchain, ... }:
        {
          default = pkgs.mkShell {
            packages = [
              toolchain
              pkgs.cargo-edit
            ];
          };
        }
      );

      formatter = forAllSystems ({ pkgs, ... }: pkgs.nixfmt-rfc-style);
    };
}
