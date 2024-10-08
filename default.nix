{
  pkgs ? import (fetchTarball {
    url = "https://github.com/NixOS/nixpkgs/archive/refs/tags/24.05.tar.gz";
    sha256 = "sha256:1lr1h35prqkd1mkmzriwlpvxcb34kmhc9dnr48gkm8hh089hifmx";
  }) {},
  tag ? "latest"
}:
let
  debianImage = pkgs.dockerTools.pullImage {
    imageName = "public.ecr.aws/docker/library/debian";
    imageDigest = "sha256:ad86386827b083b3d71139050b47ffb32bbd9559ea9b1345a739b14fec2d9ecf";
    sha256 = "sha256-X4mvS9xVrH7HL2ft/6XyTmMDjT7DatYoCgqPbVhFhp4=";
    finalImageTag = "12.7-slim";
    finalImageName = "debian";
  };

  nitridingDaemon = pkgs.buildGoModule {
    pname = "nitriding-daemon";
    version = "1.4.2";
    src = pkgs.fetchFromGitHub {
      owner = "brave";
      repo = "nitriding-daemon";
      rev = "v1.4.2";
      sha256 = "sha256-H1Q120Hr71hY/3Tz4d1zq4lTWew49Hbyuxu+j7HS3Wk=";
    };
    vendorHash = "sha256-t4r435eNm7lzTOctm28HML3p/i6IHapS6yzh3t07AL8=";
    CGO_ENABLED = 0;
    ldflags = ["-s" "-w"];
    checkFlags = ["-skip"];

    postInstall = ''
      mkdir -p $out/usr/local/bin
      cp $out/bin/* $out/usr/local/bin/
    '';
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

    postInstall = ''
      mkdir -p $out/usr/local/bin
      cp $out/bin/* $out/usr/local/bin/
    '';
  };

  startSh = pkgs.writeTextFile {
    name = "start.sh";
    text = builtins.readFile ./start.sh;
    executable = true;
    destination = "/usr/local/bin/start.sh";
  };

in pkgs.dockerTools.buildImage {
  name = "star-randsrv";
  tag = tag;

  fromImage = debianImage;

  config = {
    Cmd = [ "/usr/local/bin/start.sh" ];
    ExposedPorts = {
      "443/tcp" = {};
    };
    User = "65534";
  };

  copyToRoot = pkgs.buildEnv {
    name = "image-root";
    paths = [
      startSh
      nitridingDaemon
      rustApp
    ];
    pathsToLink = [ "/usr/local/bin" ];
  };
}
