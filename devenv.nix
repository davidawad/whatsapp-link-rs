{ pkgs, ... }:

{
  # https://devenv.sh/packages/
  packages = with pkgs; [
    cargo
    clippy
    rustc
    rustfmt
    just
    pre-commit
    gitleaks
  ];

  enterShell = ''
    [ -d .git ] && [ ! -f .git/hooks/commit-msg ] && pre-commit install --install-hooks --hook-type pre-commit --hook-type commit-msg --hook-type post-commit >/dev/null 2>&1 || true
  '';
}
