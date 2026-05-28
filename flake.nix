{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    flake-utils.url = "github:SpiralP/nix-flake-utils";
  };

  outputs = inputs@{ flake-utils, ... }:
    flake-utils.lib.makeOutputs inputs
      ({ lib, pkgs, makeRustPackage, dev, ... }:
        let
          # TODO remove when https://github.com/NixOS/nixpkgs/pull/524985 is
          # merged into nixos-25.11. crates.io's API rate-limits nix's curl
          # to 1 req/s and returns 403s above that; the upstream fix swaps
          # the download URL to the static.crates.io CDN. Inline the same
          # one-line change here until the backport lands.
          patchedImportCargoLock =
            let
              cargoLockDir = "${pkgs.path}/pkgs/build-support/rust";
            in
            pkgs.callPackage
              (builtins.toFile "import-cargo-lock-patched.nix"
                (builtins.replaceStrings
                  [
                    "https://crates.io/api/v1/crates"
                    "./replace-workspace-values.py"
                  ]
                  [
                    "https://static.crates.io/crates"
                    "${cargoLockDir}/replace-workspace-values.py"
                  ]
                  (builtins.readFile "${cargoLockDir}/import-cargo-lock.nix")))
              { };

          args = {
            src = ./.;

            nativeBuildInputs = with pkgs; [
              pkg-config
              rustPlatform.bindgenHook
            ];

            useNextest = true;

            cargoDeps = patchedImportCargoLock {
              lockFile = ./Cargo.lock;
              allowBuiltinFetchGit = true;
            };
          };
        in
        {
          default = makeRustPackage pkgs (self: args);
          debug = makeRustPackage pkgs (self: args // {
            buildType = "debug";
            hardeningDisable = [ "all" ];
          });
        });
}
