# Purpose: Nix derivation for the Rust `opencode` binary, the Rust counterpart of nix/opencode.nix.
# Scope: rustPlatform.buildRustPackage over the `rust/` workspace with the checked-in Cargo.lock
# (no git dependencies today, so no outputHashes are needed). Not yet wired into flake.nix: the
# flake is outside this node's ownership; wire it with
#   opencode-rust = pkgs.callPackage ./rust/distribution/nix/opencode-rust.nix { };
# Invariants: builds offline from the vendored lockfile, wraps PATH with ripgrep like the baseline.
# The baseline versionCheckHook is wired but disabled until the Rust CLI answers --version.
# Requires a nixpkgs whose rustc satisfies rust/rust-toolchain.toml (1.99) and workspace rust-version.
# Untested: nix is not installed on the agent machine (see contracts/phase-15/distribution.md).
{
  lib,
  rustPlatform,
  makeBinaryWrapper,
  installShellFiles,
  ripgrep,
  pkg-config,
  versionCheckHook,
  writableTmpDirAsHomeHook,
  stdenv,
}:
let
  root = ../..;
  workspace = lib.importTOML (root + "/Cargo.toml");
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "opencode-rust";
  version = workspace.workspace.package.version;

  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.difference root (
      lib.fileset.unions [
        (lib.fileset.maybeMissing (root + "/target"))
        (lib.fileset.maybeMissing (root + "/spikes"))
      ]
    );
  };

  cargoLock.lockFile = root + "/Cargo.lock";

  cargoBuildFlags = [
    "--package"
    "opencode"
    "--bin"
    "opencode"
  ];
  # Workspace tests are qualified by .github/workflows/rust.yml, not inside the sandbox.
  doCheck = false;

  nativeBuildInputs = [
    pkg-config
    makeBinaryWrapper
    installShellFiles
  ];

  postInstall = ''
    wrapProgram $out/bin/opencode --prefix PATH : ${lib.makeBinPath [ ripgrep ]}
  ''
  + lib.optionalString (stdenv.buildPlatform.canExecute stdenv.hostPlatform) ''
    # Completions only once the Rust CLI implements `opencode completion` (Phase 09).
    if $out/bin/opencode completion bash > completion.bash 2>/dev/null; then
      installShellCompletion --cmd opencode --bash completion.bash
    fi
  '';

  nativeInstallCheckInputs = [
    versionCheckHook
    writableTmpDirAsHomeHook
  ];
  # The Rust CLI does not implement --version yet (rust/contracts/phase-09/status.md); flip to true
  # when it does so the derivation mirrors the baseline's versionCheckHook.
  doInstallCheck = false;
  versionCheckProgramArg = "--version";

  meta = {
    description = "The open source coding agent (Rust port)";
    homepage = "https://opencode.ai";
    license = lib.licenses.mit;
    mainProgram = "opencode";
    platforms = [
      "aarch64-linux"
      "x86_64-linux"
      "aarch64-darwin"
      "x86_64-darwin"
    ];
  };
})
