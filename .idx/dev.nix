{ pkgs, ... }: {
  channel = "stable-24.05";

  packages = [
    pkgs.curl
    pkgs.gcc
    pkgs.binutils
    pkgs.pkg-config
    pkgs.openssl
    pkgs.openssl.dev
    pkgs.libiconv
  ];

  env = {
    RUST_BACKTRACE = "1";
    PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
    CC = "${pkgs.gcc}/bin/gcc";
  };

  idx = {
    extensions = [
      "rust-lang.rust-analyzer"
      "tamasfe.even-better-toml"
    ];

    previews = {
      enable = false;
    };

    workspace = {
      onStart = {
        install-rust = ''
          if ! command -v cargo &> /dev/null; then
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
          fi
          source "$HOME/.cargo/env"
          rustup target add wasm32-unknown-unknown 2>/dev/null || true
        '';
      };
    };
  };
}{ pkgs, ... }: {
  channel = "stable-24.05";

  packages = [
    pkgs.gcc
    pkgs.gnumake
    pkgs.pkg-config
    pkgs.openssl
    pkgs.curl
  ];

  idx = {
    extensions = [
      "rust-lang.rust-analyzer"
    ];

    workspace = {
      onStart = {
        setup = ''
          export PATH="$HOME/.nix-profile/bin:$HOME/.cargo/bin:$PATH"
          if ! command -v cargo &> /dev/null; then
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
            source "$HOME/.cargo/env"
          fi
        '';
      };
    };
  };
}