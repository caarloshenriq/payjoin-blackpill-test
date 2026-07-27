{
  description = "payjoin-blackpill-test firmware dev shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ rust-overlay.overlays.default ];

        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # rust-toolchain.toml already declares the thumbv7em-none-eabihf
        # target and rust-src, so this pulls in exactly what's needed for
        # both the host-side build tooling and the cross build.
        embeddedRustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

        embeddedDevShell = pkgs.mkShell {
          name = "payjoin-blackpill-test-dev";
          packages = with pkgs; [
            embeddedRustToolchain
            gcc-arm-embedded
            probe-rs-tools
            dfu-util # kept available even though SWD/probe-rs is the
                     # reliable path for this board -- see project notes
          ];
          CC_thumbv7em_none_eabihf = "arm-none-eabi-gcc";
        };
      in
      {
        devShells = {
          default = embeddedDevShell;
          embedded = embeddedDevShell;
        };
      }
    );
}
