{
  description = "abmx — sync bank transaction emails into an Actual Budget ledger";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    {
      self,
      nixpkgs,
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

      # Keep the package version in sync with `[workspace.package] version`.
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};

          # The official Actual client is a Node library, so the sink ships a
          # tiny helper that abmx spawns per operation. Bundling it here keeps
          # `abmx` a single installable closure.
          actual-helper = pkgs.buildNpmPackage {
            pname = "abmx-actual-helper";
            version = (builtins.fromJSON (
              builtins.readFile ./crates/abmx-actual/helper/package.json
            )).version;

            src = ./crates/abmx-actual/helper;
            npmDepsHash = "sha256-p337IRxa7bZIXNES2cY7ZfKJMklAouJZI2tyfp9jgCo=";
            # better-sqlite3 is a native addon: build it with the same Node the
            # wrapper puts on PATH, or the ABI will not match at runtime.
            nodejs = pkgs.nodejs_22;

            # better-sqlite3's install script would try to download a prebuilt
            # binary; the sandbox has no network, so compile it from source.
            env.npm_config_build_from_source = "true";
            nativeBuildInputs = [ pkgs.python3 ];

            dontNpmBuild = true;

            installPhase = ''
              runHook preInstall
              mkdir -p $out/share/abmx/actual-helper
              cp -r . $out/share/abmx/actual-helper
              chmod +x $out/share/abmx/actual-helper/helper.js
              runHook postInstall
            '';

            meta = {
              description = "Node bridge from abmx to the official @actual-app/api client";
              license = pkgs.lib.licenses.mit;
            };
          };
        in
        {
          abmx-actual-helper = actual-helper;

          default = pkgs.rustPlatform.buildRustPackage {
            pname = "abmx";
            inherit version;

            src = ./.;

            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [
              pkgs.makeWrapper
              pkgs.pkg-config
            ];
            buildInputs = [ pkgs.sqlite ];

            # Build only the CLI. Without this the install hook also picks up
            # the dev-only `xtask` binary.
            cargoBuildFlags = [
              "--package"
              "abmx"
              "--bin"
              "abmx"
            ];

            postInstall = ''
              mkdir -p $out/share/abmx
              cp -r ${actual-helper}/share/abmx/actual-helper $out/share/abmx/actual-helper
              wrapProgram $out/bin/abmx \
                --set-default ABMX_ACTUAL_HELPER "$out/share/abmx/actual-helper/helper.js" \
                --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.nodejs_22 ]}
            '';

            # `cargo` needs a C compiler for the bundled SQLite, which stdenv
            # provides. Nothing else is required: TLS is rustls, not OpenSSL.
            doCheck = true;

            meta = {
              description = "Sync bank transaction emails into an Actual Budget ledger";
              homepage = "https://github.com/kennethlieyanto/actual-gmail-tx-sync";
              license = pkgs.lib.licenses.mit;
              mainProgram = "abmx";
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
              nodejs_22
              rustc
              rustfmt
              sqlite
            ];

            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          };
        }
      );

      checks = forAllSystems (system: {
        default = self.packages.${system}.default;
      });

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt);

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/abmx";
          meta.description = "Sync bank transaction emails into an Actual Budget ledger";
        };
      });

      nixosModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.abmx;
        in
        {
          options.services.abmx = {
            enable = lib.mkEnableOption "the abmx mail-to-ledger synchronizer";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              description = "The abmx package to run.";
            };

            configFile = lib.mkOption {
              type = lib.types.path;
              description = ''
                abmx.toml. Reference secrets through systemd credentials:
                `api_key_file = "/run/credentials/abmx.service/actual-api-key"`.
              '';
            };

            interval = lib.mkOption {
              type = lib.types.str;
              default = "15min";
              description = "How often to run a sync.";
            };

            user = lib.mkOption {
              type = lib.types.str;
              default = "abmx";
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
                /run/credentials/abmx.service/<name>.
              '';
            };

            extraArgs = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [ ];
              description = "Extra arguments appended to `abmx sync`.";
            };
          };

          config = lib.mkIf cfg.enable {
            users.groups.${cfg.user} = { };
            users.users.${cfg.user} = {
              isSystemUser = true;
              group = cfg.user;
              home = "/var/lib/abmx";
            };

            systemd.services.abmx = {
              description = "abmx — mail to ledger";
              wants = [ "network-online.target" ];
              after = [ "network-online.target" ];

              serviceConfig = {
                Type = "oneshot";
                User = cfg.user;
                Group = cfg.user;
                StateDirectory = "abmx";
                # `@actual-app/api` keeps its local copy of the budget here.
                # ProtectHome hides ~/.cache, so point it at the unit's cache.
                CacheDirectory = "abmx";
                Environment = [ "ABMX_ACTUAL_DATA_DIR=/var/cache/abmx/actual" ];
                WorkingDirectory = "/var/lib/abmx";

                # Two runs must never write to the same budget concurrently.
                ExecStart = pkgs.writeShellScript "abmx-run" ''
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

            systemd.timers.abmx = {
              description = "abmx — periodic sync";
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
