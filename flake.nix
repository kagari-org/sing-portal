{
  inputs = {
    nixpkgs.url = "nixpkgs";
    flake-parts.url = "flake-parts";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = inputs@{
    self, nixpkgs, flake-parts, rust-overlay,
  }: flake-parts.lib.mkFlake { inherit inputs; } {
    systems = [ "x86_64-linux" "aarch64-linux" ];
    perSystem = { pkgs, lib, system, ... }: let
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

      # Just the cdylib and the generated Kotlin, so a forked app can copy both
      # into its own sources.
      androidPkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
        config.allowUnfree = true;
      };

      android = let
        rust = androidPkgs.rust-bin.stable.latest.default.override {
          targets = [ "aarch64-linux-android" ];
        };
        rustPlatform = androidPkgs.makeRustPlatform { cargo = rust; rustc = rust; };
        ndk = androidPkgs.androidenv.androidPkgs.ndk-bundle;
        prebuilt = with androidPkgs.stdenv.hostPlatform;
          if isDarwin then
            (if isAarch64 then "darwin-arm64" else "darwin-x86_64")
          else
            (if isAarch64 then "linux-aarch64" else "linux-x86_64");
        toolchain = "${ndk}/libexec/android-sdk/ndk/${ndk.version}/toolchains/llvm/prebuilt/${prebuilt}/bin";
        clang = "${toolchain}/aarch64-linux-android24-clang";
      in rustPlatform.buildRustPackage {
        pname = "sing-portal-android";
        inherit version;
        src = source;
        cargoLock.lockFile = ./Cargo.lock;
        doCheck = false;
        nativeBuildInputs = [ androidPkgs.protobuf androidPkgs.ktlint ];
        CC_aarch64_linux_android = clang;
        CXX_aarch64_linux_android = "${clang}++";
        AR_aarch64_linux_android = "${toolchain}/llvm-ar";
        RANLIB_aarch64_linux_android = "${toolchain}/llvm-ranlib";
        CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = clang;
        # uniffi keeps its metadata in the library, and the release profile
        # strips it, which makes the bindgen emit nothing at all
        CARGO_PROFILE_RELEASE_STRIP = "false";
        buildPhase = ''
          runHook preBuild
          cargo build --offline --release --package sing-portal-agent-ffi --target aarch64-linux-android
          cargo build --offline --release --package sing-portal-agent-ffi --features cli --bin uniffi-bindgen
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          mkdir -p $out/jniLibs/arm64-v8a $out/kotlin
          cp target/aarch64-linux-android/release/libsing_portal_agent_ffi.so $out/jniLibs/arm64-v8a/
          target/release/uniffi-bindgen generate \
            --library $out/jniLibs/arm64-v8a/libsing_portal_agent_ffi.so \
            --language kotlin \
            --out-dir $out/kotlin
          ${toolchain}/llvm-strip --strip-unneeded $out/jniLibs/arm64-v8a/libsing_portal_agent_ffi.so
          runHook postInstall
        '';
        meta = {
          description = "sing-portal agent as an Android library";
        };
      };
    in {
      packages.web = web;
      packages.android = android;

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
          mainProgram = "sing-portal-server";
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
