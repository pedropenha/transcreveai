# Home-manager module for Transcreve.ai speech-to-text
#
# Provides a systemd user service for autostart.
# Usage: imports = [ transcreve-ai.homeManagerModules.default ];
#        services.transcreve-ai.enable = true;
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.transcreve-ai;
in
{
  options.services.transcreve-ai = {
    enable = lib.mkEnableOption "Transcreve.ai speech-to-text user service";

    package = lib.mkOption {
      type = lib.types.package;
      defaultText = lib.literalExpression "transcreve-ai.packages.\${system}.transcreve-ai";
      description = "The Transcreve.ai package to use.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.user.services.transcreve-ai = {
      Unit = {
        Description = "Transcreve.ai speech-to-text";
        After = [ "graphical-session.target" ];
        PartOf = [ "graphical-session.target" ];
      };
      Service = {
        ExecStart = "${cfg.package}/bin/transcreve-ai";
        Restart = "on-failure";
        RestartSec = 5;
      };
      Install.WantedBy = [ "graphical-session.target" ];
    };
  };
}
