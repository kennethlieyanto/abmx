{
  description = "txsync — deliver parsed bank transactions to a budget ledger";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # txp is not on crates.io yet. `flake = false` pulls the source tree so the
    # cargo build can resolve the `../txp` path dependency.
    txp = {
      url = "git+ssh://git@github.com/kennethlieyanto/txp?ref=refs/tags/v0.1.0";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      txp,
      ...
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "txsync";
            version = "0.1.0";

            src = ./.;

            cargoLock.lockFile = ./Cargo.lock;

            # The workspace depends on `../txp`. Materialise it next to the
            # build root so cargo resolves the same path it does locally.
            postPatch = ''
              mkdir -p ../txp
              cp -r ${txp}/. ../txp/
              chmod -R u+w ../txp
            '';

            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.sqlite ];

            doCheck = true;

            meta = {
              description = "Deliver parsed bank transactions to a budget ledger";
              homepage = "https://github.com/kennethlieyanto/txsync";
              license = pkgs.lib.licenses.mit;
              mainProgram = "txsync";
            };
          };
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              just
              rustc
              rustfmt
              sqlite
            ];

            # `cargo` needs a C compiler for the bundled SQLite, which stdenv
            # provides. Nothing else is required: TLS is rustls, not OpenSSL.
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          };
        }
      );

      nixosModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.txsync;
        in
        {
          options.services.txsync = {
            enable = lib.mkEnableOption "the txsync mail-to-ledger synchronizer";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              description = "The txsync package to run.";
            };

            configFile = lib.mkOption {
              type = lib.types.path;
              description = ''
                txsync.toml. Reference secrets through systemd credentials:
                `api_key_file = "/run/credentials/txsync.service/actual-api-key"`.
              '';
            };

            interval = lib.mkOption {
              type = lib.types.str;
              default = "15min";
              description = "How often to run a sync.";
            };

            user = lib.mkOption {
              type = lib.types.str;
              default = "txsync";
              description = "User the service runs as.";
            };

            secrets = lib.mkOption {
              type = lib.types.attrsOf lib.types.path;
              default = { };
              example = {
                "actual-api-key" = "/run/secrets/actual-api-key";
                "gmail-client-secret" = "/run/secrets/gmail-client-secret";
              };
              description = ''
                Files exposed to the service as systemd credentials under
                /run/credentials/txsync.service/<name>.
              '';
            };

            extraArgs = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [ ];
              description = "Extra arguments appended to `txsync sync`.";
            };
          };

          config = lib.mkIf cfg.enable {
            users.groups.${cfg.user} = { };
            users.users.${cfg.user} = {
              isSystemUser = true;
              group = cfg.user;
              home = "/var/lib/txsync";
            };

            systemd.services.txsync = {
              description = "txsync — mail to ledger";
              wants = [ "network-online.target" ];
              after = [ "network-online.target" ];

              serviceConfig = {
                Type = "oneshot";
                User = cfg.user;
                Group = cfg.user;
                StateDirectory = "txsync";
                WorkingDirectory = "/var/lib/txsync";

                # Two runs must never write to the same budget concurrently.
                ExecStart = pkgs.writeShellScript "txsync-run" ''
                  set -euo pipefail
                  exec ${lib.getExe cfg.package} --config ${cfg.configFile} sync ${lib.escapeShellArgs cfg.extraArgs}
                '';

                LoadCredential = lib.mapAttrsToList (name: path: "${name}:${path}") cfg.secrets;

                # The only writable path is the state directory.
                ProtectSystem = "strict";
                ProtectHome = true;
                PrivateTmp = true;
                NoNewPrivileges = true;
                RestrictAddressFamilies = [
                  "AF_INET"
                  "AF_INET6"
                  "AF_UNIX"
                ];
              };
            };

            systemd.timers.txsync = {
              description = "txsync — periodic sync";
              wantedBy = [ "timers.target" ];
              timerConfig = {
                OnBootSec = "2min";
                OnUnitActiveSec = cfg.interval;
                Persistent = true;
                RandomizedDelaySec = "30s";
              };
            };
          };
        };
    };
}
