{
  description = "A very basic flake";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-24.05";
    nitriding.url = "github:brave/nitriding-daemon/255fa70056b35b86ea493a0157d7a0b49a82579b";
  };

  outputs = { self, nixpkgs, nitriding }:
    let
      tag = "latest";
      system = "x86_64-linux";

      pkgs = import nixpkgs {
        inherit system;
      };

      startSh = pkgs.writeTextFile {
        name = "start.sh";
        text = builtins.readFile ./start.sh;
        executable = true;
        destination = "/bin/start.sh";
      };

    in rec {
      dockerImage = pkgs.dockerTools.buildImage {
        name = "star-randsrv";
        tag = tag;

        config = {
          Cmd = [ "/bin/start.sh" ];
          ExposedPorts = {
            "443/tcp" = {};
          };
          User = "65534";
        };

        copyToRoot = pkgs.buildEnv {
          name = "image-root";
          paths = [
            pkgs.bash
            pkgs.coreutils
            startSh
            nitriding.outputs.packages.${system}.default
            rustApp
          ];
          pathsToLink = [ "/bin" ];
        };
      };
      rustApp = pkgs.rustPlatform.buildRustPackage {
        pname = "star-randsrv";
        version = "0.2.0";

        src = builtins.filterSource
          (path: type:
            let relPath = pkgs.lib.removePrefix (toString ./. + "/") path;
            in (relPath == "src" && type == "directory") || pkgs.lib.hasSuffix ".rs" relPath ||
                relPath == "Cargo.toml" || relPath == "Cargo.lock")
          ./.;
        cargoLock = {
          lockFile = ./Cargo.lock;
        };
      };

      packages.x86_64-linux.default = dockerImage;
    };
}
