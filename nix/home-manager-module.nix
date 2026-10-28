self: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.bac-calendar;
in {
  options.services.bac-calendar = {
    enable = lib.mkEnableOption "a systemd user timer that syncs FCBQ matches into Google calendars";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "bac-calendar.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "The bac-calendar package to install and run.";
    };

    schedule = lib.mkOption {
      type = lib.types.str;
      default = "00/3:00";
      example = "hourly";
      description = "When to sync, as a systemd calendar expression; see {manpage}`systemd.time(7)`.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [cfg.package];

    systemd.user.services.bac-calendar = {
      Unit.Description = "Sync FCBQ matches into Google Calendar";
      Service = {
        Type = "oneshot";
        ExecStart = "${lib.getExe cfg.package} sync";
      };
    };

    systemd.user.timers.bac-calendar = {
      Unit.Description = "Sync FCBQ matches into Google Calendar periodically";
      Timer = {
        OnCalendar = cfg.schedule;
        Persistent = true;
        RandomizedDelaySec = "10min";
      };
      Install.WantedBy = ["timers.target"];
    };
  };
}
