{
  description = "Cider Cluster Console development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      system = "aarch64-darwin";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          cargo
          rustc
          rustfmt
          clippy
          rust-analyzer
          pkg-config
          python3
          nixfmt
        ];

        buildInputs = [ pkgs.openssl ];

        # Let rust-analyzer find the standard library from the pinned toolchain.
        RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
      };

      formatter.${system} = pkgs.nixfmt;
    };
}
