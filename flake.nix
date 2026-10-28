{
  description = "Syncs basketball teams' FCBQ matches into Google calendars";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  outputs = {
    self,
    nixpkgs,
  }: let
    systems = ["x86_64-linux"];
    forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
  in {
    packages = forAllSystems (pkgs: {
      default = pkgs.callPackage ./nix/package.nix {};
    });

    formatter = forAllSystems (pkgs:
      pkgs.writeShellApplication {
        name = "alejandra";
        runtimeInputs = [pkgs.alejandra];
        text = ''
          if [ $# -eq 0 ]; then set -- .; fi
          exec alejandra "$@"
        '';
      });

    homeManagerModules.default = import ./nix/home-manager-module.nix self;
  };
}
