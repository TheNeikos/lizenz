{
  description = "The lizenz project flake";
  inputs = {
    nixpkgs.url = "nixpkgs/nixos-25.05";
    flake-utils = {
      url = "github:numtide/flake-utils";
    };
    crane = {
      url = "github:ipetkov/crane";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
  };

  outputs =
    inputs:
    inputs.flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import inputs.nixpkgs {
          inherit system;
          overlays = [ (import inputs.rust-overlay) ];
        };

        rustTarget = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        unstableRustTarget = pkgs.rust-bin.selectLatestNightlyWith (
          toolchain:
          toolchain.default.override {
            extensions = [
              "rust-src"
              "miri"
              "rustfmt"
            ];
          }
        );
        craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustTarget;
        unstableCraneLib = (inputs.crane.mkLib pkgs).overrideToolchain unstableRustTarget;

        tomlInfo = craneLib.crateNameFromCargoToml { cargoToml = ./Cargo.toml; };
        inherit (tomlInfo) version;
        src = ./.;

        rustfmt' = pkgs.writeShellScriptBin "rustfmt" ''
          exec "${unstableRustTarget}/bin/rustfmt" "$@"
        '';

        cargoArtifacts = craneLib.buildDepsOnly {
          inherit src;
          cargoExtraArgs = "--all-features --all";
        };

        crate = craneLib.buildPackage {
          inherit cargoArtifacts src version;
          cargoExtraArgs = "--all-features --all";
        };

      in
      rec {
        checks = {
          inherit crate;

          crate-clippy = craneLib.cargoClippy {
            inherit cargoArtifacts src;
            cargoExtraArgs = "--all --all-features";
            cargoClippyExtraArgs = "-- --deny warnings";
          };

          crate-fmt = unstableCraneLib.cargoFmt {
            inherit src;
          };
        };

        packages.crate = crate;
        packages.default = packages.crate;

        apps.crate = inputs.flake-utils.lib.mkApp {
          name = "library";
          drv = crate;
        };
        apps.default = apps.crate;

        devShells.default = devShells.crate;
        devShells.crate = pkgs.mkShell {
          buildInputs = [ ];

          nativeBuildInputs = [
            rustfmt'
            rustTarget
          ];

          TREE_SITTER_GRAMMARS = pkgs.tree-sitter.withPlugins (_: pkgs.tree-sitter.allGrammars);
        };
      }
    );
}
