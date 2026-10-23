{
  outputs = inputs@{
    self, nixpkgs, flake-parts,
  }: flake-parts.lib.mkFlake { inherit inputs; } {
    systems = [ "x86_64-linux" "aarch64-linux" ];
    perSystem = { pkgs, lib, ... }: let
      version = "0.1.0";

      webDeps = pkgs.stdenvNoCC.mkDerivation {
        pname = "sing-portal-web-deps";
        inherit version;
        src = lib.fileset.toSource {
          root = ./web;
          fileset = lib.fileset.unions [
            ./web/package.json
            ./web/yarn.lock
            ./web/.yarnrc.yml
            ./web/.yarn
          ];
        };
        nativeBuildInputs = [ pkgs.nodejs ];
        outputHashMode = "recursive";
        outputHash = "sha256-zQP10657GILjJIYVPyrbPvQyPL3NGM6ZgzDFQghLlu4=";
        buildPhase = ''
          runHook preBuild
          export HOME=$(mktemp -d)
          export SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt
          export NIX_SSL_CERT_FILE=$SSL_CERT_FILE
          export NODE_EXTRA_CA_CERTS=$SSL_CERT_FILE
          node .yarn/releases/yarn-4.18.0.cjs install --immutable
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          rm -f node_modules/.yarn-state.yml
          mkdir -p $out
          cp -a node_modules $out/node_modules
          runHook postInstall
        '';
        dontFixup = true;
      };

      web = pkgs.stdenvNoCC.mkDerivation {
        pname = "sing-portal-web";
        inherit version;
        src = lib.fileset.toSource {
          root = ./web;
          fileset = lib.fileset.unions [
            ./web/package.json
            ./web/yarn.lock
            ./web/.yarnrc.yml
            ./web/.yarn
            ./web/index.html
            ./web/tsconfig.json
            ./web/vite.config.ts
            ./web/src
          ];
        };
        nativeBuildInputs = [ pkgs.nodejs ];
        buildPhase = ''
          runHook preBuild
          cp -a ${webDeps}/node_modules node_modules
          node node_modules/vue-tsc/bin/vue-tsc.js --noEmit
          node node_modules/vite/bin/vite.js build
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          mkdir -p $out
          cp -a dist/. $out/
          runHook postInstall
        '';
      };

      source = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./crates
          ./migrations
        ];
      };
    in {
      packages.web = web;

      packages.default = pkgs.rustPlatform.buildRustPackage {
        pname = "sing-portal";
        inherit version;
        src = source;
        cargoLock.lockFile = ./Cargo.lock;
        doCheck = false;
        nativeBuildInputs = [ pkgs.protobuf ];
        postPatch = ''
          mkdir -p web/dist
          cp -a ${web}/. web/dist/
        '';
        meta = {
          description = "sing-box subscription portal and node agent";
          mainProgram = "sing-portal";
        };
      };

      devShells.default = pkgs.mkShell {
        packages = with pkgs; [
          rustc
          cargo
          protobuf
          clippy
          rustfmt
          nodejs
          sqlite
          jq
        ];
      };
    };
  };
}
