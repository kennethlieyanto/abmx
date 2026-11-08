{
  description = "txp — parse Indonesian bank transaction emails into structured JSON";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f {
            inherit system;
            pkgs = import nixpkgs { inherit system; };
          }
        );

      # Keep the package version in sync with `[workspace.package] version`.
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
    in
    {
      packages = forAllSystems (
        { pkgs, ... }:
        let
          txp = pkgs.rustPlatform.buildRustPackage {
            pname = "txp";
            inherit version;

            src = self;
            cargoLock.lockFile = ./Cargo.lock;

            # Build only the CLI. Without this the install hook also picks up
            # the dev-only `xtask` binary.
            cargoBuildFlags = [
              "--package"
              "txp"
              "--bin"
              "txp"
            ];

            nativeBuildInputs = [ pkgs.makeWrapper ];

            postInstall = ''
              mkdir -p $out/share/txp
              cp -r parsers $out/share/txp/parsers
              wrapProgram $out/bin/txp \
                --set-default TXP_PARSERS "$out/share/txp/parsers"
            '';

            meta = {
              description = "Parse Indonesian bank transaction emails into structured JSON";
              license = pkgs.lib.licenses.mit;
              mainProgram = "txp";
              platforms = pkgs.lib.platforms.linux;
            };
          };
        in
        {
          inherit txp;
          default = txp;
        }
      );

      devShells = forAllSystems (
        { pkgs, ... }: {
          default = pkgs.mkShellNoCC {
            packages = with pkgs; [
              cargo
              rustc
              clippy
              rustfmt
              just
            ];

            # Keep `just txp ...` fast by preferring the debug build.
            shellHook = ''
              export PATH="$PWD/target/debug:$PATH"
            '';
          };
        }
      );

      checks = forAllSystems (
        { system, ... }: {
          default = self.packages.${system}.txp;
        }
      );

      formatter = forAllSystems ({ pkgs, ... }: pkgs.nixfmt);

      apps = forAllSystems (
        { system, ... }: {
          default = {
            type = "app";
            program = "${self.packages.${system}.txp}/bin/txp";
            meta.description = "Parse Indonesian bank transaction emails into structured JSON";
          };
        }
      );
    };
}
